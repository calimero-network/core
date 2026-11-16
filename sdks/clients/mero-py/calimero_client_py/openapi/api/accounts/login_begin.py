from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.login_begin_body import LoginBeginBody
from ...models.login_begin_response_200 import LoginBeginResponse200
from typing import cast


def _get_kwargs(
    *,
    body: LoginBeginBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/auth/passkey/login/begin",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, LoginBeginResponse200]]:
    if response.status_code == 200:
        response_200 = LoginBeginResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 429:
        response_429 = Error.from_dict(response.json())

        return response_429

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, LoginBeginResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: LoginBeginBody,
) -> Response[Union[Error, LoginBeginResponse200]]:
    """Start a sign-in

     A WebAuthn challenge for any passkey of the cluster. `origin` is an app frontend's registered
    origin, whose rp-id the ceremony then uses. `options` are the WebAuthn options to hand to the
    browser as they are: free-form here, their shape is WebAuthn's.

    **Who:** anyone.

    Args:
        body (LoginBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, LoginBeginResponse200]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    body: LoginBeginBody,
) -> Optional[Union[Error, LoginBeginResponse200]]:
    """Start a sign-in

     A WebAuthn challenge for any passkey of the cluster. `origin` is an app frontend's registered
    origin, whose rp-id the ceremony then uses. `options` are the WebAuthn options to hand to the
    browser as they are: free-form here, their shape is WebAuthn's.

    **Who:** anyone.

    Args:
        body (LoginBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, LoginBeginResponse200]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: LoginBeginBody,
) -> Response[Union[Error, LoginBeginResponse200]]:
    """Start a sign-in

     A WebAuthn challenge for any passkey of the cluster. `origin` is an app frontend's registered
    origin, whose rp-id the ceremony then uses. `options` are the WebAuthn options to hand to the
    browser as they are: free-form here, their shape is WebAuthn's.

    **Who:** anyone.

    Args:
        body (LoginBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, LoginBeginResponse200]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: LoginBeginBody,
) -> Optional[Union[Error, LoginBeginResponse200]]:
    """Start a sign-in

     A WebAuthn challenge for any passkey of the cluster. `origin` is an app frontend's registered
    origin, whose rp-id the ceremony then uses. `options` are the WebAuthn options to hand to the
    browser as they are: free-form here, their shape is WebAuthn's.

    **Who:** anyone.

    Args:
        body (LoginBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, LoginBeginResponse200]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
