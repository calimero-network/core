from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.confirm_only import ConfirmOnly
from ...models.error import Error
from ...models.public_link_revoke_response_200 import PublicLinkRevokeResponse200
from typing import cast


def _get_kwargs(
    app: str,
    link: str,
    *,
    body: ConfirmOnly,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/apps/{app}/public-links/{link}".format(
            app=app,
            link=link,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PublicLinkRevokeResponse200]]:
    if response.status_code == 200:
        response_200 = PublicLinkRevokeResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

    if response.status_code == 412:
        response_412 = Error.from_dict(response.json())

        return response_412

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, PublicLinkRevokeResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    link: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Response[Union[Error, PublicLinkRevokeResponse200]]:
    """Revoke a public link

     Its creator, or whoever may change the instance's access; it admits nobody from the next request.
    `412` first.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        link (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PublicLinkRevokeResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        link=link,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    link: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Optional[Union[Error, PublicLinkRevokeResponse200]]:
    """Revoke a public link

     Its creator, or whoever may change the instance's access; it admits nobody from the next request.
    `412` first.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        link (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PublicLinkRevokeResponse200]
    """

    return sync_detailed(
        app=app,
        link=link,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    app: str,
    link: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Response[Union[Error, PublicLinkRevokeResponse200]]:
    """Revoke a public link

     Its creator, or whoever may change the instance's access; it admits nobody from the next request.
    `412` first.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        link (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PublicLinkRevokeResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        link=link,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    link: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Optional[Union[Error, PublicLinkRevokeResponse200]]:
    """Revoke a public link

     Its creator, or whoever may change the instance's access; it admits nobody from the next request.
    `412` first.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        link (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PublicLinkRevokeResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            link=link,
            client=client,
            body=body,
        )
    ).parsed
