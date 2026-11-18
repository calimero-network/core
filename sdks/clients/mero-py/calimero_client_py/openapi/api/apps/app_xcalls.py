from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_xcalls_response_200 import AppXcallsResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/xcalls".format(
            app=app,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppXcallsResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppXcallsResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 503:
        response_503 = Error.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AppXcallsResponse200, Error]]:
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
) -> Response[Union[AppXcallsResponse200, Error]]:
    """An app's cross-app calls

     The owner or an organization admin: how many calls wait in its outbox for delivery (`pending`, a
    count), and its dead letters, oldest first (`atMs`: the entry time it was dead-lettered at, Unix
    milliseconds).

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppXcallsResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppXcallsResponse200, Error]]:
    """An app's cross-app calls

     The owner or an organization admin: how many calls wait in its outbox for delivery (`pending`, a
    count), and its dead letters, oldest first (`atMs`: the entry time it was dead-lettered at, Unix
    milliseconds).

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppXcallsResponse200, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppXcallsResponse200, Error]]:
    """An app's cross-app calls

     The owner or an organization admin: how many calls wait in its outbox for delivery (`pending`, a
    count), and its dead letters, oldest first (`atMs`: the entry time it was dead-lettered at, Unix
    milliseconds).

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppXcallsResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppXcallsResponse200, Error]]:
    """An app's cross-app calls

     The owner or an organization admin: how many calls wait in its outbox for delivery (`pending`, a
    count), and its dead letters, oldest first (`atMs`: the entry time it was dead-lettered at, Unix
    milliseconds).

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppXcallsResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
        )
    ).parsed
