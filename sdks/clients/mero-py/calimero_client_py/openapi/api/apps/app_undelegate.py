from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_undelegate_response_200 import AppUndelegateResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
    target: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/apps/{app}/delegations/{target}".format(
            app=app,
            target=target,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppUndelegateResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppUndelegateResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

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
) -> Response[Union[AppUndelegateResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    target: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppUndelegateResponse200, Error]]:
    """Take back what an app may do on another as you

     No passkey: it only narrows. `404 NOT_DELEGATED` when you granted nothing there.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        target (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppUndelegateResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        target=target,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    target: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppUndelegateResponse200, Error]]:
    """Take back what an app may do on another as you

     No passkey: it only narrows. `404 NOT_DELEGATED` when you granted nothing there.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        target (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppUndelegateResponse200, Error]
    """

    return sync_detailed(
        app=app,
        target=target,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    target: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppUndelegateResponse200, Error]]:
    """Take back what an app may do on another as you

     No passkey: it only narrows. `404 NOT_DELEGATED` when you granted nothing there.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        target (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppUndelegateResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        target=target,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    target: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppUndelegateResponse200, Error]]:
    """Take back what an app may do on another as you

     No passkey: it only narrows. `404 NOT_DELEGATED` when you granted nothing there.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        target (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppUndelegateResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            target=target,
            client=client,
        )
    ).parsed
