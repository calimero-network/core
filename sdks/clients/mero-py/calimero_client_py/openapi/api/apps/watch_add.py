from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.watch_add_body import WatchAddBody
from ...models.watch_added import WatchAdded
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: WatchAddBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/watches".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, WatchAdded]]:
    if response.status_code == 200:
        response_200 = WatchAdded.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

    if response.status_code == 413:
        response_413 = Error.from_dict(response.json())

        return response_413

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 422:
        response_422 = Error.from_dict(response.json())

        return response_422

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
) -> Response[Union[Error, WatchAdded]]:
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
    body: WatchAddBody,
) -> Response[Union[Error, WatchAdded]]:
    """Watch a subscribable view

     Runs `view` (a view the app's ABI marks `subscribable`) as the caller, with `/views`' gate, records
    what it read and keeps it on `stream` (a watch stream of `/events?watch=true`): the stream then
    carries this watch's new answer, `sub`, when an entry that writes what it read (or, for a view that
    read the directory or the clock, a system change) changes it. `index` is the app's last entry in the
    state it read and `answer` its answer. The view runs again at most once per the interval its app
    sets (`watchIntervalMs` in the ABI, default 1000); `intervalMs` can only lengthen it, up to 10000. A
    stream holds at most 64 watches and a node 2,048; `args` is at most 4 KiB.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):
        body (WatchAddBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, WatchAdded]]
    """

    kwargs = _get_kwargs(
        app=app,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
    body: WatchAddBody,
) -> Optional[Union[Error, WatchAdded]]:
    """Watch a subscribable view

     Runs `view` (a view the app's ABI marks `subscribable`) as the caller, with `/views`' gate, records
    what it read and keeps it on `stream` (a watch stream of `/events?watch=true`): the stream then
    carries this watch's new answer, `sub`, when an entry that writes what it read (or, for a view that
    read the directory or the clock, a system change) changes it. `index` is the app's last entry in the
    state it read and `answer` its answer. The view runs again at most once per the interval its app
    sets (`watchIntervalMs` in the ABI, default 1000); `intervalMs` can only lengthen it, up to 10000. A
    stream holds at most 64 watches and a node 2,048; `args` is at most 4 KiB.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):
        body (WatchAddBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, WatchAdded]
    """

    return sync_detailed(
        app=app,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
    body: WatchAddBody,
) -> Response[Union[Error, WatchAdded]]:
    """Watch a subscribable view

     Runs `view` (a view the app's ABI marks `subscribable`) as the caller, with `/views`' gate, records
    what it read and keeps it on `stream` (a watch stream of `/events?watch=true`): the stream then
    carries this watch's new answer, `sub`, when an entry that writes what it read (or, for a view that
    read the directory or the clock, a system change) changes it. `index` is the app's last entry in the
    state it read and `answer` its answer. The view runs again at most once per the interval its app
    sets (`watchIntervalMs` in the ABI, default 1000); `intervalMs` can only lengthen it, up to 10000. A
    stream holds at most 64 watches and a node 2,048; `args` is at most 4 KiB.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):
        body (WatchAddBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, WatchAdded]]
    """

    kwargs = _get_kwargs(
        app=app,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
    body: WatchAddBody,
) -> Optional[Union[Error, WatchAdded]]:
    """Watch a subscribable view

     Runs `view` (a view the app's ABI marks `subscribable`) as the caller, with `/views`' gate, records
    what it read and keeps it on `stream` (a watch stream of `/events?watch=true`): the stream then
    carries this watch's new answer, `sub`, when an entry that writes what it read (or, for a view that
    read the directory or the clock, a system change) changes it. `index` is the app's last entry in the
    state it read and `answer` its answer. The view runs again at most once per the interval its app
    sets (`watchIntervalMs` in the ABI, default 1000); `intervalMs` can only lengthen it, up to 10000. A
    stream holds at most 64 watches and a node 2,048; `args` is at most 4 KiB.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):
        body (WatchAddBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, WatchAdded]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
