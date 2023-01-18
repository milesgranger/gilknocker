import pytest
import numpy as np
import threading
import time
from gilknocker import KnockKnock


N_THREADS = 4
N_PTS = 4096


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
    print(f"Metric: {knocker.contention_metric}")
    return knocker


def test_knockknock_busy():
    knocker = _run(a_lotta_gil)

    try:
        # usually ~0.9, but sometimes ~0.6 on Mac
        assert knocker.contention_metric > 0.6

        # Now wait for it to 'cool' back down
        # by looping over some work which releases the GIL
        prev_cm = knocker.contention_metric
        for i in range(10):
            a_little_gil()
            assert knocker.contention_metric < prev_cm
            prev_cm = knocker.contention_metric

        # ~0.15 oN mY MaChInE.
        assert knocker.contention_metric < 0.3
    finally:
        knocker.stop()


def test_knockknock_available_gil():
    knocker = _run(a_little_gil)

    try:
        # usually ~0.002
        assert knocker.contention_metric < 0.06
    finally:
        knocker.stop()


def test_knockknock_reset_contention_metric():
    knocker = _run(a_lotta_gil)

    try:
        assert knocker.contention_metric > 0.6
        knocker.reset_contention_metric()
        assert knocker.contention_metric < 0.001

    finally:
        knocker.stop()


# Manual verification with py-spy
# busy should give high GIL %
if __name__ == "__main__":
    test_knockknock_busy()
