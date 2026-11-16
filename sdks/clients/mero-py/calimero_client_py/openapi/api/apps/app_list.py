from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_list_response_200 import AppListResponse200
from ...models.error import Error
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    *,
    package: Union[Unset, str] = UNSET,
    bundle: Union[Unset, str] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["package"] = package

    params["bundle"] = bundle

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppListResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppListResponse200.from_dict(response.json())

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
) -> Response[Union[AppListResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    package: Union[Unset, str] = UNSET,
    bundle: Union[Unset, str] = UNSET,
) -> Response[Union[AppListResponse200, Error]]:
    """Apps the caller reaches

     Anyone may list public apps.

    **Who:** signed in (a session or an API key).

    Args:
        package (Union[Unset, str]):
        bundle (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppListResponse200, Error]]
    """

    kwargs = _get_kwargs(
        package=package,
        bundle=bundle,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    package: Union[Unset, str] = UNSET,
    bundle: Union[Unset, str] = UNSET,
) -> Optional[Union[AppListResponse200, Error]]:
    """Apps the caller reaches

     Anyone may list public apps.

    **Who:** signed in (a session or an API key).

    Args:
        package (Union[Unset, str]):
        bundle (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppListResponse200, Error]
    """

    return sync_detailed(
        client=client,
        package=package,
        bundle=bundle,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    package: Union[Unset, str] = UNSET,
    bundle: Union[Unset, str] = UNSET,
) -> Response[Union[AppListResponse200, Error]]:
    """Apps the caller reaches

     Anyone may list public apps.

    **Who:** signed in (a session or an API key).

    Args:
        package (Union[Unset, str]):
        bundle (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppListResponse200, Error]]
    """

    kwargs = _get_kwargs(
        package=package,
        bundle=bundle,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    package: Union[Unset, str] = UNSET,
    bundle: Union[Unset, str] = UNSET,
) -> Optional[Union[AppListResponse200, Error]]:
    """Apps the caller reaches

     Anyone may list public apps.

    **Who:** signed in (a session or an API key).

    Args:
        package (Union[Unset, str]):
        bundle (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppListResponse200, Error]
    """

    return (
        await asyncio_detailed(
            client=client,
            package=package,
            bundle=bundle,
        )
    ).parsed
