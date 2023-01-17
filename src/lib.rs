#[deny(missing_docs)]
use pyo3::prelude::*;
use pyo3::{
    exceptions::{PyBrokenPipeError, PyTimeoutError, PyValueError},
    PyResult,
};
use std::{
    mem::take,
    sync::mpsc::{channel, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

#[pymodule]
fn gilknocker(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<KnockKnock>()?;
    Ok(())
}

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
    handle: Option<thread::JoinHandle<f32>>,
    channel: Option<Sender<bool>>,
    contention_metric: Option<f32>,
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
    pub fn contention_metric(&self) -> Option<f32> {
        self.contention_metric
    }

    /// Start polling the GIL to check if it's locked.
    pub fn start(&mut self, py: Python) -> () {
        let (send, recv) = channel();
        self.channel = Some(send);

        let interval = self.interval;
        let handle = py.allow_threads(move || {
            thread::spawn(move || {
                let mut time_to_acquire = Duration::from_millis(0);
                let runtime = Instant::now();
                while recv
                    .recv_timeout(interval)
                    .unwrap_or_else(|e| e != RecvTimeoutError::Disconnected)
                {
                    let start = Instant::now();
                    time_to_acquire += Python::with_gil(move |_py| start.elapsed());
                }
                time_to_acquire.as_micros() as f32 / runtime.elapsed().as_micros() as f32
            })
        });
        self.handle = Some(handle);
    }

    /// Stop polling the GIL.
    pub fn stop(&mut self) -> PyResult<()> {
        match take(&mut self.handle) {
            Some(handle) => {
                if let Some(send) = take(&mut self.channel) {
                    send.send(false)
                        .map_err(|e| PyBrokenPipeError::new_err(e.to_string()))?;

                    let start = Instant::now();
                    while !handle.is_finished() {
                        thread::sleep(Duration::from_millis(100));
                        if start.elapsed() > self.timeout {
                            return Err(PyTimeoutError::new_err("Failed to join knocker thread."));
                        }
                    }
                }
                self.contention_metric = handle.join().ok();
                Ok(())
            }
            None => Err(PyValueError::new_err(
                "Appears `start` was not called, no handle.",
            )),
        }
    }
}
