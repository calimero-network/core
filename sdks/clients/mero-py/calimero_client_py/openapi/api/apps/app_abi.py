from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_abi import AppAbi
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/abi".format(
            app=app,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppAbi, Error]]:
    if response.status_code == 200:
        response_200 = AppAbi.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AppAbi, Error]]:
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
) -> Response[Union[AppAbi, Error]]:
    """An app's ABI

     The ABI its bundle embeds: methods, events, errors and state. `abi` is the bundle's own document,
    free-form here: its format is the ABI's, not this API's.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppAbi, Error]]
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
) -> Optional[Union[AppAbi, Error]]:
    """An app's ABI

     The ABI its bundle embeds: methods, events, errors and state. `abi` is the bundle's own document,
    free-form here: its format is the ABI's, not this API's.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppAbi, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppAbi, Error]]:
    """An app's ABI

     The ABI its bundle embeds: methods, events, errors and state. `abi` is the bundle's own document,
    free-form here: its format is the ABI's, not this API's.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppAbi, Error]]
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
) -> Optional[Union[AppAbi, Error]]:
    """An app's ABI

     The ABI its bundle embeds: methods, events, errors and state. `abi` is the bundle's own document,
    free-form here: its format is the ABI's, not this API's.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppAbi, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
        )
    ).parsed
