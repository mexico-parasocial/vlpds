"""Share stage.sh's exclusive-operation protocol without taking over an active operation."""
from contextlib import contextmanager
import os


@contextmanager
def operation(state):
    lock = state / 'operation.lock'
    try:
        lock.mkdir(mode=0o700)
    except FileExistsError:
        try:
            pid = int((lock / 'pid').read_text())
            if pid <= 0: raise ValueError()
            os.kill(pid, 0)
        except ProcessLookupError:
            (lock / 'pid').unlink()
            lock.rmdir()
            lock.mkdir(mode=0o700)
        else:
            raise SystemExit('Another staging operation is active')
    try:
        (lock / 'pid').write_text(str(os.getpid()))
        yield
    finally:
        (lock / 'pid').unlink()
        lock.rmdir()
