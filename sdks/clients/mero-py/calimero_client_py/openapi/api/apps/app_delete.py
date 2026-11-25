from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_deleted import AppDeleted
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/apps/{app}".format(
            app=app,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppDeleted, Error]]:
    if response.status_code == 200:
        response_200 = AppDeleted.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 409:
        response_409 = Error.from_dict(response.json())

        return response_409

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AppDeleted, Error]]:
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
) -> Response[Union[AppDeleted, Error]]:
    """Delete an app

     The owner or an organization admin. An instance whose release created apps with it (`uses` with
    `mode: create`, its `links`) is deleted with every one of them, theirs included, listed in
    `members`; a member is not deleted alone while its instance stands (`409 UNIT_MEMBER`), and an
    instance with members waits for every node to run 1.0.0-rc.32 (`409 FEATURE_NOT_ACTIVE`).

    **Who:** a passkey session (an API key is refused).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppDeleted, Error]]
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
) -> Optional[Union[AppDeleted, Error]]:
    """Delete an app

     The owner or an organization admin. An instance whose release created apps with it (`uses` with
    `mode: create`, its `links`) is deleted with every one of them, theirs included, listed in
    `members`; a member is not deleted alone while its instance stands (`409 UNIT_MEMBER`), and an
    instance with members waits for every node to run 1.0.0-rc.32 (`409 FEATURE_NOT_ACTIVE`).

    **Who:** a passkey session (an API key is refused).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppDeleted, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppDeleted, Error]]:
    """Delete an app

     The owner or an organization admin. An instance whose release created apps with it (`uses` with
    `mode: create`, its `links`) is deleted with every one of them, theirs included, listed in
    `members`; a member is not deleted alone while its instance stands (`409 UNIT_MEMBER`), and an
    instance with members waits for every node to run 1.0.0-rc.32 (`409 FEATURE_NOT_ACTIVE`).

    **Who:** a passkey session (an API key is refused).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppDeleted, Error]]
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
) -> Optional[Union[AppDeleted, Error]]:
    """Delete an app

     The owner or an organization admin. An instance whose release created apps with it (`uses` with
    `mode: create`, its `links`) is deleted with every one of them, theirs included, listed in
    `members`; a member is not deleted alone while its instance stands (`409 UNIT_MEMBER`), and an
    instance with members waits for every node to run 1.0.0-rc.32 (`409 FEATURE_NOT_ACTIVE`).

    **Who:** a passkey session (an API key is refused).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppDeleted, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
        )
    ).parsed
