from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.recovery_start_body import RecoveryStartBody
from ...models.recovery_start_response_200 import RecoveryStartResponse200
from typing import cast


def _get_kwargs(
    org: str,
    *,
    body: RecoveryStartBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/orgs/{org}/recovery/start".format(
            org=org,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, RecoveryStartResponse200]]:
    if response.status_code == 200:
        response_200 = RecoveryStartResponse200.from_dict(response.json())

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
) -> Response[Union[Error, RecoveryStartResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
    body: RecoveryStartBody,
) -> Response[Union[Error, RecoveryStartResponse200]]:
    """Start an organization recovery

     The recovery seed's signature handing the organization to the caller.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        body (RecoveryStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, RecoveryStartResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    org: str,
    *,
    client: AuthenticatedClient,
    body: RecoveryStartBody,
) -> Optional[Union[Error, RecoveryStartResponse200]]:
    """Start an organization recovery

     The recovery seed's signature handing the organization to the caller.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        body (RecoveryStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, RecoveryStartResponse200]
    """

    return sync_detailed(
        org=org,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
    body: RecoveryStartBody,
) -> Response[Union[Error, RecoveryStartResponse200]]:
    """Start an organization recovery

     The recovery seed's signature handing the organization to the caller.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        body (RecoveryStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, RecoveryStartResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    org: str,
    *,
    client: AuthenticatedClient,
    body: RecoveryStartBody,
) -> Optional[Union[Error, RecoveryStartResponse200]]:
    """Start an organization recovery

     The recovery seed's signature handing the organization to the caller.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        body (RecoveryStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, RecoveryStartResponse200]
    """

    return (
        await asyncio_detailed(
            org=org,
            client=client,
            body=body,
        )
    ).parsed
