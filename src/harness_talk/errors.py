"""Failure codes recorded in notification, cleanup and discovery results.

htalk builds each code from constants and validated values. Any other exception
is recorded by class name only, so foreign error text, paths and credentials
never reach the shared database or command output through these fields.
"""


class Coded:
    """Marks an exception whose text is a fixed htalk code."""


class CodedValueError(Coded, ValueError):
    pass


class CodedOSError(Coded, OSError):
    pass


class CodedTimeoutError(Coded, TimeoutError):
    pass


def failure_detail(exc):
    return str(exc) if isinstance(exc, Coded) else type(exc).__name__
