from concurrent.futures import ThreadPoolExecutor, wait
from gilknocker import KnockKnock, lock_and_release_gil
import time


def test_knockknock():
    knocker = KnockKnock(5)
    assert knocker.time_locked_ms() is None

    n = 10
    knocker.start()
    jobs = []
    with ThreadPoolExecutor(10) as executor:
        for _ in range(n):
            jobs.append(executor.submit(lock_and_release_gil, 0, 5000))
        wait(jobs)
    knocker.stop()
    assert knocker.time_unlocked_ms() > 0
    breakpoint()