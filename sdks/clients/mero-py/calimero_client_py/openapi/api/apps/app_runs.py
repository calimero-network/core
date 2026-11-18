from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_runs_response_200 import AppRunsResponse200
from ...models.error import Error
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    app: str,
    *,
    limit: Union[Unset, str] = UNSET,
    before: Union[Unset, str] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["limit"] = limit

    params["before"] = before

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/runs".format(
            app=app,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppRunsResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppRunsResponse200.from_dict(response.json())

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

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AppRunsResponse200, Error]]:
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
    limit: Union[Unset, str] = UNSET,
    before: Union[Unset, str] = UNSET,
) -> Response[Union[AppRunsResponse200, Error]]:
    """An app's compute runs

     Newest first, `limit` (1 to 100, default 20) at a time; `next` is the `before` of the next page,
    `null` on the last.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        limit (Union[Unset, str]):
        before (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppRunsResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        limit=limit,
        before=before,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
    limit: Union[Unset, str] = UNSET,
    before: Union[Unset, str] = UNSET,
) -> Optional[Union[AppRunsResponse200, Error]]:
    """An app's compute runs

     Newest first, `limit` (1 to 100, default 20) at a time; `next` is the `before` of the next page,
    `null` on the last.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        limit (Union[Unset, str]):
        before (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppRunsResponse200, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
        limit=limit,
        before=before,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
    limit: Union[Unset, str] = UNSET,
    before: Union[Unset, str] = UNSET,
) -> Response[Union[AppRunsResponse200, Error]]:
    """An app's compute runs

     Newest first, `limit` (1 to 100, default 20) at a time; `next` is the `before` of the next page,
    `null` on the last.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        limit (Union[Unset, str]):
        before (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppRunsResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        limit=limit,
        before=before,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
    limit: Union[Unset, str] = UNSET,
    before: Union[Unset, str] = UNSET,
) -> Optional[Union[AppRunsResponse200, Error]]:
    """An app's compute runs

     Newest first, `limit` (1 to 100, default 20) at a time; `next` is the `before` of the next page,
    `null` on the last.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        limit (Union[Unset, str]):
        before (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppRunsResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            limit=limit,
            before=before,
        )
    ).parsed
