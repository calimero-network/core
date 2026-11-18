from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.passkey_label_body import PasskeyLabelBody
from ...models.passkey_label_response_200 import PasskeyLabelResponse200
from typing import cast


def _get_kwargs(
    cred: str,
    *,
    body: PasskeyLabelBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/auth/passkeys/{cred}".format(
            cred=cred,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PasskeyLabelResponse200]]:
    if response.status_code == 200:
        response_200 = PasskeyLabelResponse200.from_dict(response.json())

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

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, PasskeyLabelResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    cred: str,
    *,
    client: AuthenticatedClient,
    body: PasskeyLabelBody,
) -> Response[Union[Error, PasskeyLabelResponse200]]:
    """Name a passkey

     **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        cred (str):
        body (PasskeyLabelBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PasskeyLabelResponse200]]
    """

    kwargs = _get_kwargs(
        cred=cred,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    cred: str,
    *,
    client: AuthenticatedClient,
    body: PasskeyLabelBody,
) -> Optional[Union[Error, PasskeyLabelResponse200]]:
    """Name a passkey

     **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        cred (str):
        body (PasskeyLabelBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PasskeyLabelResponse200]
    """

    return sync_detailed(
        cred=cred,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    cred: str,
    *,
    client: AuthenticatedClient,
    body: PasskeyLabelBody,
) -> Response[Union[Error, PasskeyLabelResponse200]]:
    """Name a passkey

     **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        cred (str):
        body (PasskeyLabelBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PasskeyLabelResponse200]]
    """

    kwargs = _get_kwargs(
        cred=cred,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    cred: str,
    *,
    client: AuthenticatedClient,
    body: PasskeyLabelBody,
) -> Optional[Union[Error, PasskeyLabelResponse200]]:
    """Name a passkey

     **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        cred (str):
        body (PasskeyLabelBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PasskeyLabelResponse200]
    """

    return (
        await asyncio_detailed(
            cred=cred,
            client=client,
            body=body,
        )
    ).parsed
