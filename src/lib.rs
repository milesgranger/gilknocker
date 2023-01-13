use std::{
    mem::take,
    sync::mpsc::{channel, Sender},
    thread,
    time::Duration,
};

use pyo3::prelude::*;
use pyo3::{
    exceptions::{PyBrokenPipeError, PyValueError},
    ffi::{PyGILState_Check, PyGILState_STATE},
    PyResult,
};

#[pymodule]
fn gilknocker(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_function(wrap_pyfunction!(lock_and_release_gil, m)?)?;
    m.add_class::<KnockKnock>()?;
    Ok(())
}

pub type Milliseconds = u128;

/// Struct for polling, knocking on the GIL,
/// checking if it's locked in the current thread
#[pyclass(name = "KnockKnock")]
#[derive(Default)]
pub struct KnockKnock {
    handle: Option<thread::JoinHandle<(Milliseconds, Milliseconds)>>,
    channel: Option<Sender<bool>>,
    time_locked_ms: Option<Milliseconds>,
    time_unlocked_ms: Option<Milliseconds>,
    monitor_interval: u64,
}

#[pymethods]
impl KnockKnock {
    #[new]
    pub fn __init__(monitor_interval_ms: u64) -> PyResult<Self> {
        Ok(KnockKnock {
            monitor_interval: monitor_interval_ms,
            ..Default::default()
        })
    }
    /// Get time locked
    pub fn time_locked_ms(&self) -> Option<Milliseconds> {
        self.time_locked_ms
    }
    /// Get time unlocked
    pub fn time_unlocked_ms(&self) -> Option<Milliseconds> {
        self.time_unlocked_ms
    }
    /// Start polling the GIL to check if it's locked.
    pub fn start(&mut self, py: Python) -> () {
        let (send, recv) = channel();
        self.channel = Some(send);

        let interval = self.monitor_interval;

        let handle = py.allow_threads(move || {
            thread::spawn(move || {
                let mut time_locked_ms = 0;
                let mut time_unlocked_ms = 0;
                let duration = Duration::from_millis(interval);
                loop {
                    if let Ok(stop) = recv.try_recv() {
                        if stop {
                            break;
                        }
                    }
                    unsafe {
                        if PyGILState_Check() == PyGILState_STATE::PyGILState_LOCKED as i32 {
                            time_locked_ms += duration.as_millis();
                        } else {
                            time_unlocked_ms += duration.as_millis();
                        }
                    }
                    thread::sleep(duration);
                }
                (time_locked_ms, time_unlocked_ms)
            })
        });
        self.handle = Some(handle);
    }

    /// Stop polling the GIL.
    pub fn stop(&mut self) -> PyResult<()> {
        // Kill loop
        if let Some(send) = &self.channel {
            send.send(true)
                .map_err(|e| PyBrokenPipeError::new_err(e.to_string()))?
        }
        self.channel = None;

        // Recv time locked
        match take(&mut self.handle) {
            Some(handle) => {
                (self.time_locked_ms, self.time_unlocked_ms) = handle
                    .join()
                    .map(|(locked, unlocked)| (Some(locked), Some(unlocked)))
                    .unwrap();
                Ok(())
            }
            None => Err(PyValueError::new_err(
                "Appears `start` was not called, no handle.",
            )),
        }
    }
}

/// Lock and release the GIL in one function; for sanity check of GIL state monitoring.
#[pyfunction]
pub fn lock_and_release_gil(py: Python<'_>, lock_for_ms: u64, release_for_ms: u64) {
    thread::sleep(Duration::from_millis(lock_for_ms));
    py.allow_threads(move || {
        // let handle = thread::spawn(move || {
        //     thread::sleep(Duration::from_millis(release_for_ms));
        // });
        // handle.join().unwrap();
        let _ = ack(4, 1);
    });
}

fn ack(n: u64, m: u64) -> u64 {
    if n == 0 {
        m + 1
    } else if m == 0 {
        ack(n - 1, 1)
    } else {
        ack(n - 1, ack(n, m - 1))
    }
}
