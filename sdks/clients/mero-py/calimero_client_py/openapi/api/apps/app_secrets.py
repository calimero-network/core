from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_secrets_response_200 import AppSecretsResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/secrets".format(
            app=app,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppSecretsResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppSecretsResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

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
) -> Response[Union[AppSecretsResponse200, Error]]:
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
) -> Response[Union[AppSecretsResponse200, Error]]:
    """The secrets an app's release declares

     For each secret the release the instance runs declares (its signed manifest's `secrets`): its hosts,
    description and whether it is required, and `source`: `app` when the instance has its own value
    (with its `fingerprint`, the first 16 hex digits of its SHA-256, `setBy` and `setAt`), else
    `cluster` (the operator's, if it bound one). Never a value. The owner, or whoever manages the
    organization's apps.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppSecretsResponse200, Error]]
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
) -> Optional[Union[AppSecretsResponse200, Error]]:
    """The secrets an app's release declares

     For each secret the release the instance runs declares (its signed manifest's `secrets`): its hosts,
    description and whether it is required, and `source`: `app` when the instance has its own value
    (with its `fingerprint`, the first 16 hex digits of its SHA-256, `setBy` and `setAt`), else
    `cluster` (the operator's, if it bound one). Never a value. The owner, or whoever manages the
    organization's apps.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppSecretsResponse200, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppSecretsResponse200, Error]]:
    """The secrets an app's release declares

     For each secret the release the instance runs declares (its signed manifest's `secrets`): its hosts,
    description and whether it is required, and `source`: `app` when the instance has its own value
    (with its `fingerprint`, the first 16 hex digits of its SHA-256, `setBy` and `setAt`), else
    `cluster` (the operator's, if it bound one). Never a value. The owner, or whoever manages the
    organization's apps.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppSecretsResponse200, Error]]
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
) -> Optional[Union[AppSecretsResponse200, Error]]:
    """The secrets an app's release declares

     For each secret the release the instance runs declares (its signed manifest's `secrets`): its hosts,
    description and whether it is required, and `source`: `app` when the instance has its own value
    (with its `fingerprint`, the first 16 hex digits of its SHA-256, `setBy` and `setAt`), else
    `cluster` (the operator's, if it bound one). Never a value. The owner, or whoever manages the
    organization's apps.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppSecretsResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
        )
    ).parsed
