from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_origin_add_body import AppOriginAddBody
from ...models.error import Error
from ...models.package_origin import PackageOrigin
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: AppOriginAddBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/origins".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PackageOrigin]]:
    if response.status_code == 200:
        response_200 = PackageOrigin.from_dict(response.json())

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

    if response.status_code == 409:
        response_409 = Error.from_dict(response.json())

        return response_409

    if response.status_code == 412:
        response_412 = Error.from_dict(response.json())

        return response_412

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 429:
        response_429 = Error.from_dict(response.json())

        return response_429

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, PackageOrigin]]:
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
    body: AppOriginAddBody,
) -> Response[Union[Error, PackageOrigin]]:
    """Register a web origin for an instance

     By whoever manages the instance (its owner or an admin of its organization), with a passkey step-up
    over `{ origin, rpId }`. A passkey ceremony there opens a session bound to the instance's package
    that reaches this instance alone; what the account may do in it is the instance's grants'. Answers
    the origin as registered (the one already there, when the same origin is registered again).

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppOriginAddBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PackageOrigin]]
    """

    kwargs = _get_kwargs(
        app=app,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
    body: AppOriginAddBody,
) -> Optional[Union[Error, PackageOrigin]]:
    """Register a web origin for an instance

     By whoever manages the instance (its owner or an admin of its organization), with a passkey step-up
    over `{ origin, rpId }`. A passkey ceremony there opens a session bound to the instance's package
    that reaches this instance alone; what the account may do in it is the instance's grants'. Answers
    the origin as registered (the one already there, when the same origin is registered again).

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppOriginAddBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PackageOrigin]
    """

    return sync_detailed(
        app=app,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
    body: AppOriginAddBody,
) -> Response[Union[Error, PackageOrigin]]:
    """Register a web origin for an instance

     By whoever manages the instance (its owner or an admin of its organization), with a passkey step-up
    over `{ origin, rpId }`. A passkey ceremony there opens a session bound to the instance's package
    that reaches this instance alone; what the account may do in it is the instance's grants'. Answers
    the origin as registered (the one already there, when the same origin is registered again).

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppOriginAddBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PackageOrigin]]
    """

    kwargs = _get_kwargs(
        app=app,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
    body: AppOriginAddBody,
) -> Optional[Union[Error, PackageOrigin]]:
    """Register a web origin for an instance

     By whoever manages the instance (its owner or an admin of its organization), with a passkey step-up
    over `{ origin, rpId }`. A passkey ceremony there opens a session bound to the instance's package
    that reaches this instance alone; what the account may do in it is the instance's grants'. Answers
    the origin as registered (the one already there, when the same origin is registered again).

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppOriginAddBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PackageOrigin]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
