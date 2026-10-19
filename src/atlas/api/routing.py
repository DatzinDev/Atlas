"""Translate Atlas failures only on routes owned by the library."""

from collections.abc import Callable, Coroutine
from typing import Any

from fastapi import Request, Response
from fastapi.responses import JSONResponse
from fastapi.routing import APIRoute

from atlas.core.exceptions import AtlasError, error_response


class AtlasRoute(APIRoute):
    def get_route_handler(self) -> Callable[[Request], Coroutine[Any, Any, Response]]:
        handler = super().get_route_handler()

        async def handle(request: Request) -> Response:
            try:
                return await handler(request)
            except AtlasError as exc:
                return JSONResponse(error_response(exc), status_code=exc.status_code)

        return handle
