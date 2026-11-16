from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.usage_response_200 import UsageResponse200
from typing import cast


def _get_kwargs() -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/auth/usage",
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, UsageResponse200]]:
    if response.status_code == 200:
        response_200 = UsageResponse200.from_dict(response.json())

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
) -> Response[Union[Error, UsageResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, UsageResponse200]]:
    """The account's usage against its quotas

     The apps the account owns, its bundle bytes (a decimal string), API keys and owned organizations;
    the file bytes held by each payer it belongs to (each organization it is in, and `account:<id>` for
    its own apps); the cluster's quotas; and what today's runs of the apps the account develops used
    (decimal strings).

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UsageResponse200]]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, UsageResponse200]]:
    """The account's usage against its quotas

     The apps the account owns, its bundle bytes (a decimal string), API keys and owned organizations;
    the file bytes held by each payer it belongs to (each organization it is in, and `account:<id>` for
    its own apps); the cluster's quotas; and what today's runs of the apps the account develops used
    (decimal strings).

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UsageResponse200]
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, UsageResponse200]]:
    """The account's usage against its quotas

     The apps the account owns, its bundle bytes (a decimal string), API keys and owned organizations;
    the file bytes held by each payer it belongs to (each organization it is in, and `account:<id>` for
    its own apps); the cluster's quotas; and what today's runs of the apps the account develops used
    (decimal strings).

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UsageResponse200]]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, UsageResponse200]]:
    """The account's usage against its quotas

     The apps the account owns, its bundle bytes (a decimal string), API keys and owned organizations;
    the file bytes held by each payer it belongs to (each organization it is in, and `account:<id>` for
    its own apps); the cluster's quotas; and what today's runs of the apps the account develops used
    (decimal strings).

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UsageResponse200]
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
