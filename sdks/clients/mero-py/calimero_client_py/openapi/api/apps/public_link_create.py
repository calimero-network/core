from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.public_link_create_body import PublicLinkCreateBody
from ...models.public_link_create_response_200 import PublicLinkCreateResponse200
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: PublicLinkCreateBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/public-links".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PublicLinkCreateResponse200]]:
    if response.status_code == 200:
        response_200 = PublicLinkCreateResponse200.from_dict(response.json())

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
) -> Response[Union[Error, PublicLinkCreateResponse200]]:
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
    body: PublicLinkCreateBody,
) -> Response[Union[Error, PublicLinkCreateResponse200]]:
    r"""Make a public link of an instance

     A link whose token lets anyone call the instance's `#[app::public]` methods as an anonymous visitor
    (`Caller::Visitor { link, key }`), whatever its access: send `Authorization: Bearer
    vis1.<token>.<visitor key>.<at>.<signature>`, the visitor's own Ed25519 key (64 hex) signing
    `\"calimero.visitor.v1\0\" ‖ link ‖ 0 ‖ key ‖ 0 ‖ at` (hex) with `at` within 5 minutes of the node's
    clock. Whoever may change the instance's access, while its organization allows public links (`409
    PUBLIC_LINKS_OFF`). Not on an instance set `public` before 1.0.0-rc.32 (`409 LEGACY_PUBLIC`): it
    admits callers without an account already, and a link would make its access a grant list, which does
    not. `ratePerMinute` (1 to 600, default 60) bounds each visitor's calls on each node; `expiresAt`
    (Unix seconds) is optional. The token is answered once; the log keeps only its public key (`link`).
    At most 16 per instance. `412` first; `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.32.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (PublicLinkCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PublicLinkCreateResponse200]]
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
    body: PublicLinkCreateBody,
) -> Optional[Union[Error, PublicLinkCreateResponse200]]:
    r"""Make a public link of an instance

     A link whose token lets anyone call the instance's `#[app::public]` methods as an anonymous visitor
    (`Caller::Visitor { link, key }`), whatever its access: send `Authorization: Bearer
    vis1.<token>.<visitor key>.<at>.<signature>`, the visitor's own Ed25519 key (64 hex) signing
    `\"calimero.visitor.v1\0\" ‖ link ‖ 0 ‖ key ‖ 0 ‖ at` (hex) with `at` within 5 minutes of the node's
    clock. Whoever may change the instance's access, while its organization allows public links (`409
    PUBLIC_LINKS_OFF`). Not on an instance set `public` before 1.0.0-rc.32 (`409 LEGACY_PUBLIC`): it
    admits callers without an account already, and a link would make its access a grant list, which does
    not. `ratePerMinute` (1 to 600, default 60) bounds each visitor's calls on each node; `expiresAt`
    (Unix seconds) is optional. The token is answered once; the log keeps only its public key (`link`).
    At most 16 per instance. `412` first; `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.32.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (PublicLinkCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PublicLinkCreateResponse200]
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
    body: PublicLinkCreateBody,
) -> Response[Union[Error, PublicLinkCreateResponse200]]:
    r"""Make a public link of an instance

     A link whose token lets anyone call the instance's `#[app::public]` methods as an anonymous visitor
    (`Caller::Visitor { link, key }`), whatever its access: send `Authorization: Bearer
    vis1.<token>.<visitor key>.<at>.<signature>`, the visitor's own Ed25519 key (64 hex) signing
    `\"calimero.visitor.v1\0\" ‖ link ‖ 0 ‖ key ‖ 0 ‖ at` (hex) with `at` within 5 minutes of the node's
    clock. Whoever may change the instance's access, while its organization allows public links (`409
    PUBLIC_LINKS_OFF`). Not on an instance set `public` before 1.0.0-rc.32 (`409 LEGACY_PUBLIC`): it
    admits callers without an account already, and a link would make its access a grant list, which does
    not. `ratePerMinute` (1 to 600, default 60) bounds each visitor's calls on each node; `expiresAt`
    (Unix seconds) is optional. The token is answered once; the log keeps only its public key (`link`).
    At most 16 per instance. `412` first; `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.32.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (PublicLinkCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PublicLinkCreateResponse200]]
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
    body: PublicLinkCreateBody,
) -> Optional[Union[Error, PublicLinkCreateResponse200]]:
    r"""Make a public link of an instance

     A link whose token lets anyone call the instance's `#[app::public]` methods as an anonymous visitor
    (`Caller::Visitor { link, key }`), whatever its access: send `Authorization: Bearer
    vis1.<token>.<visitor key>.<at>.<signature>`, the visitor's own Ed25519 key (64 hex) signing
    `\"calimero.visitor.v1\0\" ‖ link ‖ 0 ‖ key ‖ 0 ‖ at` (hex) with `at` within 5 minutes of the node's
    clock. Whoever may change the instance's access, while its organization allows public links (`409
    PUBLIC_LINKS_OFF`). Not on an instance set `public` before 1.0.0-rc.32 (`409 LEGACY_PUBLIC`): it
    admits callers without an account already, and a link would make its access a grant list, which does
    not. `ratePerMinute` (1 to 600, default 60) bounds each visitor's calls on each node; `expiresAt`
    (Unix seconds) is optional. The token is answered once; the log keeps only its public key (`link`).
    At most 16 per instance. `412` first; `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.32.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (PublicLinkCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PublicLinkCreateResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
