import pytest
import numpy as np
import threading
from gilknocker import KnockKnock


def a_lotta_gil():
    """Keep the GIL busy"""
    for i in range(100_000_000):
        pass


def a_little_gil():
    """Work which releases the GIL"""
    for i in range(2):
        x = np.random.randn(4096, 4096)
        x[:] = np.fft.fft2(x).real


@pytest.mark.parametrize("interval", (None, 10, 100, 1_000, 10_000, 100_000))
@pytest.mark.parametrize("target", (a_lotta_gil, a_little_gil))
def test_bench(benchmark, interval: int, target):
    if interval:
        knocker = KnockKnock(interval)
        knocker.start()

    benchmark(target)

    if interval:
        knocker.stop()
