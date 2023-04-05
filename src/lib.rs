#[deny(missing_docs)]
use parking_lot::{const_rwlock, RwLock};
use pyo3::ffi::{PyEval_InitThreads, PyEval_ThreadsInitialized};
use pyo3::prelude::*;
use pyo3::PyResult;
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
    polling_interval: Duration,
    sampling_interval: Duration,
    sleeping_interval: Duration,
    timeout: Duration,
}

#[pymethods]
impl KnockKnock {
    /// Initialize with ``polling_interval_micros``, as the time between trying to acquire the GIL,
    /// ``sampling_interval_micros`` as the time between the polling routine, and ``timeout_secs``
    /// as time to wait for monitoring thread to exit.
    ///
    /// A more frequent polling interval will give a more accurate reflection of actual GIL contention,
    /// and a more frequent sampling interval will increase the 'real time' reflection of GIL contention.
    /// Alternatively a less frequent sampling interval will come to reflect an average GIL contention of
    /// the running program.
    ///
    /// polling_interval_micros: Optional[int]
    ///     How frequently to ask to aquire the GIL, defaults to 1_000 microseconds (1ms)
    /// sampling_interval_micros: Optional[int]
    ///     How long to sample the GIL contention at polling interval,
    ///     defaults to 10x polling_interval_micros.
    /// sleeping_interval_micros: Optional[int]
    ///     How long to sleep without sampling the GIL, defaults to 100x polling_interval_micros.
    /// timeout_micros: Optional[int]
    ///     Timeout when attempting to stop or send messages to monitoring thread. Defaults to
    ///     max(sleeping_interval_micros, sampling_interval_micros, polling_interval_micros) + 1ms
    #[new]
    pub fn __new__(
        polling_interval_micros: Option<u64>,
        sampling_interval_micros: Option<u64>,
        sleeping_interval_micros: Option<u64>,
        timeout_micros: Option<u64>,
    ) -> PyResult<Self> {
        let polling_interval =
            Duration::from_micros(polling_interval_micros.unwrap_or_else(|| 1000));
        let sampling_interval = Duration::from_micros(
            sampling_interval_micros.unwrap_or_else(|| polling_interval.as_micros() as u64 * 10),
        );
        let sleeping_interval = Duration::from_micros(
            sleeping_interval_micros.unwrap_or_else(|| polling_interval.as_micros() as u64 * 100),
        );
        let timeout = match timeout_micros {
            Some(micros) => Duration::from_micros(micros),
            None => Duration::from_micros(
                std::cmp::max(sampling_interval.as_micros(), sleeping_interval.as_micros()) as u64
                    + 1_000,
            ),
        };
        Ok(KnockKnock {
            polling_interval,
            sampling_interval,
            sleeping_interval,
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
    pub fn reset_contention_metric(&mut self, py: Python) -> PyResult<()> {
        if let Some(tx) = &self.tx {
            // notify thread to reset metric and timers
            if let Err(e) = tx.send(Message::Reset) {
                let warning = py.get_type::<pyo3::exceptions::PyUserWarning>();
                PyErr::warn(py, warning, &e.to_string(), 0)?;
            }

            // wait for ack
            if let Err(e) = self
                .rx
                .as_ref()
                .unwrap() // if tx is set, then rx is as well.
                .recv_timeout(self.timeout)
            {
                let warning = py.get_type::<pyo3::exceptions::PyUserWarning>();
                PyErr::warn(py, warning, &e.to_string(), 0)?;
            }
        }
        *(*self.contention_metric).write() = 0f32;
        Ok(())
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

        let polling_interval = self.polling_interval;
        let sampling_interval = self.sampling_interval;
        let sleeping_interval = self.sleeping_interval;

        let handle = py.allow_threads(move || {
            thread::spawn(move || {
                let mut total_time_waiting = Duration::from_millis(0);
                let mut total_time_sampling = Duration::from_millis(0);

                let sample_gil = || {
                    thread::spawn(move || {
                        let time_sampling = Instant::now();
                        let mut time_waiting = Duration::from_secs(0);

                        // Begin polling gil for duration of sampling interval
                        while time_sampling.elapsed() < sampling_interval {
                            let start = Instant::now();
                            time_waiting += Python::with_gil(move |_| start.elapsed());
                            thread::sleep(polling_interval);
                        }
                        (time_waiting, time_sampling.elapsed())
                    })
                };

                let mut handle = Some(sample_gil());
                loop {
                    match recv.recv_timeout(sleeping_interval) {
                        Ok(message) => match message {
                            Message::Stop => break,
                            Message::Reset => {
                                total_time_waiting = Duration::from_millis(0);
                                total_time_sampling = Duration::from_millis(0);
                                *(*contention_metric).write() = 0_f32;
                                send.send(Ack).unwrap(); // notify reset done
                            }
                        },
                        Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {
                            if handle
                                .as_ref()
                                .map(|hdl| hdl.is_finished())
                                .unwrap_or_else(|| false)
                            {
                                let (time_waiting, time_sampling) =
                                    take(&mut handle).unwrap().join().unwrap();
                                total_time_sampling += time_sampling;
                                total_time_waiting += time_waiting;
                                let mut cm = (*contention_metric).write();
                                *cm = total_time_waiting.as_micros() as f32
                                    / total_time_sampling.as_micros() as f32;
                                debug_assert!(handle.is_none()); // handle reset when done
                            } else if handle.is_none() {
                                handle = Some(sample_gil());
                            }
                        }
                    }
                }
            })
        });
        self.handle = Some(handle);
    }

    /// Is the GIL knocker thread running?
    #[getter]
    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Stop polling the GIL.
    pub fn stop(&mut self, py: Python) -> PyResult<()> {
        if let Some(handle) = take(&mut self.handle) {
            if let Some(send) = take(&mut self.tx) {
                if let Err(e) = send.send(Message::Stop) {
                    let warning = py.get_type::<pyo3::exceptions::PyUserWarning>();
                    PyErr::warn(py, warning, &e.to_string(), 0)?;
                }

                let start = Instant::now();
                while !handle.is_finished() {
                    if start.elapsed() > self.timeout {
                        let warning = py.get_type::<pyo3::exceptions::PyUserWarning>();
                        PyErr::warn(py, warning, "Timed out waiting for sampling thread.", 0)?;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            handle.join().ok(); // Just ignore any potential panic from sampling thread.
        }
        Ok(())
    }
}
