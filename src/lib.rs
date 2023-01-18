#[deny(missing_docs)]
use parking_lot::{const_rwlock, RwLock};
use pyo3::ffi::{PyEval_InitThreads, PyEval_ThreadsInitialized};
use pyo3::prelude::*;
use pyo3::{
    exceptions::{PyBrokenPipeError, PyRuntimeError, PyTimeoutError, PyValueError},
    PyResult,
};
use std::{
    mem::take,
    sync::{
        mpsc::{channel, Receiver, RecvTimeoutError, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[pymodule]
fn gilknocker(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<KnockKnock>()?;
    Ok(())
}

/// Possible messages to pass to the monitoring thread.
enum Message {
    Stop,
    Reset,
}

/// Acknowledgement from monitoring thread
struct Ack;

/// Struct for polling, knocking on the GIL,
/// checking if it's locked in the current thread
///
/// Example
/// -------
/// ```python
/// from gilknocker import KnockKnock
/// knocker = KnockKnock(100)  # try to reacquire the gil every 100 microseconds
/// knocker.start()
/// ... some smart code ...
/// knocker.stop()
/// knocker.contention_metric  # float between 0-1 indicating GIL contention
/// ```
#[pyclass(name = "KnockKnock")]
#[derive(Default)]
pub struct KnockKnock {
    handle: Option<thread::JoinHandle<()>>,
    tx: Option<Sender<Message>>,
    rx: Option<Receiver<Ack>>,
    contention_metric: Arc<RwLock<f32>>,
    interval: Duration,
    timeout: Duration,
}

#[pymethods]
impl KnockKnock {
    /// Initialize with interval (microseconds), as the time between trying to acquire the GIL,
    /// and timeout (seconds) as time to wait for monitoring thread to exit.
    #[new]
    pub fn __new__(interval_micros: Option<u64>, timeout_secs: Option<u64>) -> PyResult<Self> {
        let interval = Duration::from_micros(interval_micros.unwrap_or_else(|| 10));
        let timeout = Duration::from_secs(timeout_secs.unwrap_or_else(|| 5));
        if timeout <= interval {
            return Err(PyValueError::new_err(format!(
                "`interval` ({:?}) must be less than `timeout` ({:?})",
                interval, timeout
            )));
        }
        Ok(KnockKnock {
            interval,
            timeout,
            ..Default::default()
        })
    }

    /// Get the contention metric, not _specific_ meaning other than a higher
    /// value (closer to 1) indicates increased contention when acquiring the GIL.
    /// and lower indicates less contention, with 0 theoretically indicating zero
    /// contention.
    #[getter]
    pub fn contention_metric(&self) -> f32 {
        *(*self.contention_metric).read()
    }

    /// Reset the contention metric/monitoring state
    pub fn reset_contention_metric(&mut self) -> PyResult<()> {
        match &self.tx {
            Some(tx) => {
                // notify thread to reset metric and timers
                tx.send(Message::Reset)
                    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

                // wait for ack
                self.rx
                    .as_ref()
                    .unwrap() // if tx is set, then rx is as well.
                    .recv_timeout(self.timeout)
                    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
                Ok(())
            }
            None => Err(PyValueError::new_err(
                "Does not appear `start` was called, nothing to reset.",
            )),
        }
    }

    /// Start polling the GIL to check if it's locked.
    pub fn start(&mut self, py: Python) -> () {
        unsafe {
            if PyEval_ThreadsInitialized() == 0 {
                PyEval_InitThreads();
            }
        }

        // send messages to thread
        let (tx, recv) = channel();
        self.tx = Some(tx);

        // recieve messages from thread
        let (send, rx) = channel();
        self.rx = Some(rx);

        let contention_metric = Arc::new(const_rwlock(0_f32));
        self.contention_metric = contention_metric.clone();

        let interval = self.interval;
        let handle = py.allow_threads(move || {
            thread::spawn(move || {
                let mut time_to_acquire = Duration::from_millis(0);
                let mut runtime = Instant::now();
                let mut handle: Option<thread::JoinHandle<Duration>> = None;
                loop {
                    match recv.recv_timeout(interval) {
                        Ok(message) => match message {
                            Message::Stop => break,
                            Message::Reset => {
                                time_to_acquire = Duration::from_millis(0);
                                runtime = Instant::now();
                                *(*contention_metric).write() = 0_f32;
                                send.send(Ack).unwrap(); // notify reset done
                            }
                        },
                        Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => match handle {
                            Some(hdl) => {
                                if hdl.is_finished() {
                                    time_to_acquire += hdl.join().unwrap();
                                    let mut cm = (*contention_metric).write();
                                    *cm = time_to_acquire.as_micros() as f32
                                        / runtime.elapsed().as_micros() as f32;
                                    handle = None;
                                } else {
                                    handle = Some(hdl);
                                }
                            }
                            None => {
                                handle = Some(thread::spawn(move || {
                                    let start = Instant::now();
                                    Python::with_gil(move |_py| start.elapsed())
                                }));
                            }
                        },
                    }
                }
            })
        });
        self.handle = Some(handle);
    }

    /// Stop polling the GIL.
    pub fn stop(&mut self) -> PyResult<()> {
        match take(&mut self.handle) {
            Some(handle) => {
                if let Some(send) = take(&mut self.tx) {
                    send.send(Message::Stop)
                        .map_err(|e| PyBrokenPipeError::new_err(e.to_string()))?;

                    let start = Instant::now();
                    while !handle.is_finished() {
                        thread::sleep(Duration::from_millis(100));
                        if start.elapsed() > self.timeout {
                            return Err(PyTimeoutError::new_err("Failed to stop knocker thread."));
                        }
                    }
                }
                handle
                    .join()
                    .map_err(|_| PyRuntimeError::new_err("Failed to join knocker thread."))?;
                Ok(())
            }
            None => Err(PyValueError::new_err(
                "Appears `start` was not called, no handle.",
            )),
        }
    }
}
