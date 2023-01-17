## GIL Knocker


`pip install gilknocker`


[![Code Style](https://img.shields.io/badge/code%20style-black-000000.svg)](https://github.com/python/black)
[![CI](https://github.com/milesgranger/gilknocker/actions/workflows/CI.yml/badge.svg?branch=main)](https://github.com/milesgranger/gilknocker/actions/workflows/CI.yml)
[![PyPI](https://img.shields.io/pypi/v/gilknocker.svg)](https://pypi.org/project/gilknocker)
![PyPI - Wheel](https://img.shields.io/pypi/wheel/gilknocker)
[![Downloads](https://pepy.tech/badge/gilknocker/month)](https://pepy.tech/project/gilknocker)


When you thought the GIL was available, and you find yourself suspecting it might be spending time
with another. 

You probably want [py-spy](https://github.com/benfred/py-spy), however if you're
looking for a quick-and-dirty way to slip in a GIL contention metric within a specific
chunk of code, this might help you.

### How?

Unfortunately, there doesn't appear to be any explicit C-API for checking how busy
the GIL is. [PyGILState_Check](https://docs.python.org/3/c-api/init.html#c.PyGILState_Check) 
won't really work, that's limited to the current thread. 
[PyInterpreterState](https://docs.python.org/3/c-api/init.html#c.PyGILState_Check) 
is an opaque struct, and the [PyRuntimeState](https://github.com/python/cpython/blob/main/Include/internal/pycore_pystate.h)
and other goodies are private in CPython.

So, in ~200 lines of Rusty code, I've conjured up a basic metric that seems 
to align with what is reported by `py-spy` when running the same [test case](./tests/test_knockknock.py).
This works by spawning a thread which, at regular intervals, re-acquires the GIL and checks 
how long it took for the GIL to answer.

Note, the interval (`interval_micros`) is configurable. The lower the value, the 
more accurate the metric, but will be more likely to cause a deadlock and/or slow your
program down.. because it will play a larger role in competing for the GIL's attention.

### Use

Look at the [tests](./tests)

```python

from gilknocker import KnockKnock

knocker = KnockKnock(interval_micros=1000, timeout_secs=1)
knocker.start()

... smart code here ...

knocker.stop()
knocker.contention_metric  # float between 0-1 indicating roughly how busy the GIL was.
```

---

### License

[Unlicense](LICENSE) or [MIT](LICENSE-MIT), at your discretion.
