"""Optional operation reports for the host application's logging hook."""

from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager, suppress
from time import perf_counter

from pydantic import BaseModel

from atlas.core.exceptions import AtlasError


class OperationLog(BaseModel):
    layer: str | None
    operation: str
    execution_time: float = 0
    feature_count: int | None = None
    user: str | None = None
    error: str | None = None
    request: str | None = None


LogHook = Callable[[OperationLog], None]


@asynccontextmanager
async def log_operation(
    hook: LogHook | None, operation: str, layer: str | None = None, user: str | None = None
) -> AsyncIterator[OperationLog]:
    report = OperationLog(layer=layer, operation=operation, user=user)
    if hook is None:
        yield report
        return
    started = perf_counter()
    try:
        yield report
    except BaseException as exc:
        report.error = exc.code if isinstance(exc, AtlasError) else type(exc).__name__
        raise
    finally:
        report.execution_time = perf_counter() - started
        with suppress(Exception):
            hook(report)
