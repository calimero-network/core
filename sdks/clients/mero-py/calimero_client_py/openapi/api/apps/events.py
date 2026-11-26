from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    app: str,
    *,
    from_: Union[Unset, int] = UNSET,
    watch: Union[Unset, bool] = UNSET,
    stream: Union[Unset, str] = UNSET,
    last: Union[Unset, int] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["from"] = from_

    params["watch"] = watch

    params["stream"] = stream

    params["last"] = last

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/events".format(
            app=app,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, str]]:
    if response.status_code == 200:
        response_200 = response.text
        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 429:
        response_429 = Error.from_dict(response.json())

        return response_429

    if response.status_code == 503:
        response_503 = Error.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, str]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
    from_: Union[Unset, int] = UNSET,
    watch: Union[Unset, bool] = UNSET,
    stream: Union[Unset, str] = UNSET,
    last: Union[Unset, int] = UNSET,
) -> Response[Union[Error, str]]:
    """An app's events as server-sent events

     Committed events from index `from`, then live ones, each to its audience. With `watch=true`, a watch
    stream instead: its first frame is `event: stream` with `{ stream }`, then `event: view-result`
    (`id: <index>:<seq>`, `{ sub, index, answer }`, or `{ sub, index, error }` for an app error) when a
    watched view's answer changed (the first change at once, then at most one per the watch's interval),
    `event: view-changed` (`{ sub, index }`) when the new answer is over 64 KiB (read it), and `event:
    watch-ended` (`{ sub, reason }`: `access-lost`, `view-refused` or `read-set-unbounded`) when a watch
    is dropped. `stream` with `last` (or `Last-Event-ID`) resumes a stream within 30 s: each watch
    touched after `last` is sent its current answer again; a stream that is gone comes back as a new one
    followed by `event: watch-reset` (`{}`), and its watches must be added again.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):
        from_ (Union[Unset, int]):
        watch (Union[Unset, bool]):
        stream (Union[Unset, str]):
        last (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, str]]
    """

    kwargs = _get_kwargs(
        app=app,
        from_=from_,
        watch=watch,
        stream=stream,
        last=last,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
    from_: Union[Unset, int] = UNSET,
    watch: Union[Unset, bool] = UNSET,
    stream: Union[Unset, str] = UNSET,
    last: Union[Unset, int] = UNSET,
) -> Optional[Union[Error, str]]:
    """An app's events as server-sent events

     Committed events from index `from`, then live ones, each to its audience. With `watch=true`, a watch
    stream instead: its first frame is `event: stream` with `{ stream }`, then `event: view-result`
    (`id: <index>:<seq>`, `{ sub, index, answer }`, or `{ sub, index, error }` for an app error) when a
    watched view's answer changed (the first change at once, then at most one per the watch's interval),
    `event: view-changed` (`{ sub, index }`) when the new answer is over 64 KiB (read it), and `event:
    watch-ended` (`{ sub, reason }`: `access-lost`, `view-refused` or `read-set-unbounded`) when a watch
    is dropped. `stream` with `last` (or `Last-Event-ID`) resumes a stream within 30 s: each watch
    touched after `last` is sent its current answer again; a stream that is gone comes back as a new one
    followed by `event: watch-reset` (`{}`), and its watches must be added again.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):
        from_ (Union[Unset, int]):
        watch (Union[Unset, bool]):
        stream (Union[Unset, str]):
        last (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, str]
    """

    return sync_detailed(
        app=app,
        client=client,
        from_=from_,
        watch=watch,
        stream=stream,
        last=last,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
    from_: Union[Unset, int] = UNSET,
    watch: Union[Unset, bool] = UNSET,
    stream: Union[Unset, str] = UNSET,
    last: Union[Unset, int] = UNSET,
) -> Response[Union[Error, str]]:
    """An app's events as server-sent events

     Committed events from index `from`, then live ones, each to its audience. With `watch=true`, a watch
    stream instead: its first frame is `event: stream` with `{ stream }`, then `event: view-result`
    (`id: <index>:<seq>`, `{ sub, index, answer }`, or `{ sub, index, error }` for an app error) when a
    watched view's answer changed (the first change at once, then at most one per the watch's interval),
    `event: view-changed` (`{ sub, index }`) when the new answer is over 64 KiB (read it), and `event:
    watch-ended` (`{ sub, reason }`: `access-lost`, `view-refused` or `read-set-unbounded`) when a watch
    is dropped. `stream` with `last` (or `Last-Event-ID`) resumes a stream within 30 s: each watch
    touched after `last` is sent its current answer again; a stream that is gone comes back as a new one
    followed by `event: watch-reset` (`{}`), and its watches must be added again.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):
        from_ (Union[Unset, int]):
        watch (Union[Unset, bool]):
        stream (Union[Unset, str]):
        last (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, str]]
    """

    kwargs = _get_kwargs(
        app=app,
        from_=from_,
        watch=watch,
        stream=stream,
        last=last,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
    from_: Union[Unset, int] = UNSET,
    watch: Union[Unset, bool] = UNSET,
    stream: Union[Unset, str] = UNSET,
    last: Union[Unset, int] = UNSET,
) -> Optional[Union[Error, str]]:
    """An app's events as server-sent events

     Committed events from index `from`, then live ones, each to its audience. With `watch=true`, a watch
    stream instead: its first frame is `event: stream` with `{ stream }`, then `event: view-result`
    (`id: <index>:<seq>`, `{ sub, index, answer }`, or `{ sub, index, error }` for an app error) when a
    watched view's answer changed (the first change at once, then at most one per the watch's interval),
    `event: view-changed` (`{ sub, index }`) when the new answer is over 64 KiB (read it), and `event:
    watch-ended` (`{ sub, reason }`: `access-lost`, `view-refused` or `read-set-unbounded`) when a watch
    is dropped. `stream` with `last` (or `Last-Event-ID`) resumes a stream within 30 s: each watch
    touched after `last` is sent its current answer again; a stream that is gone comes back as a new one
    followed by `event: watch-reset` (`{}`), and its watches must be added again.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):
        from_ (Union[Unset, int]):
        watch (Union[Unset, bool]):
        stream (Union[Unset, str]):
        last (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, str]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            from_=from_,
            watch=watch,
            stream=stream,
            last=last,
        )
    ).parsed
