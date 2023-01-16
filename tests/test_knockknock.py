import numpy as np
import threading
import time
from gilknocker import KnockKnock


N_THREADS = 4
N_PTS = 4096


def flaky(n_tries=5):
    def wrapper(func):
        def _wrapper(*args, **kwargs):
            for _ in range(n_tries - 1):
                try:
                    return func(*args, **kwargs)
                except:
                    pass
            return func(*args, **kwargs)

        return _wrapper

    return wrapper


def a_lotta_gil():
    """Keep the GIL busy"""
    for i in range(100_000_000):
        pass


def a_little_gil():
    """Work which releases the GIL"""
    for i in range(2):
        x = np.random.randn(N_PTS, N_PTS)
        x[:] = np.fft.fft2(x).real


def _run(target):
    knocker = KnockKnock(interval_micros=1000, timeout_secs=1)
    knocker.start()
    threads = []
    for i in range(N_THREADS):
        thread = threading.Thread(target=target, daemon=True)
        threads.append(thread)
        thread.start()

    for thread in threads:
        thread.join()
    knocker.stop()
    print(f"Metric: {knocker.contention_metric}")
    return knocker


@flaky()
def test_knockknock_busy():
    knocker = _run(a_lotta_gil)
    assert knocker.contention_metric > 0.9


@flaky()
def test_knockknock_available_gil():
    knocker = _run(a_little_gil)
    assert knocker.contention_metric < 0.01


# Manual verification with py-spy
# busy should give high GIL %
if __name__ == "__main__":
    test_knockknock_busy()
