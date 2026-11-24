from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.apikey_create_body import ApikeyCreateBody
from ...models.apikey_create_response_200 import ApikeyCreateResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    *,
    body: ApikeyCreateBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/auth/apikeys",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ApikeyCreateResponse200, Error]]:
    if response.status_code == 200:
        response_200 = ApikeyCreateResponse200.from_dict(response.json())

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

    if response.status_code == 409:
        response_409 = Error.from_dict(response.json())

        return response_409

    if response.status_code == 412:
        response_412 = Error.from_dict(response.json())

        return response_412

    if response.status_code == 413:
        response_413 = Error.from_dict(response.json())

        return response_413

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 422:
        response_422 = Error.from_dict(response.json())

        return response_422

    if response.status_code == 429:
        response_429 = Error.from_dict(response.json())

        return response_429

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ApikeyCreateResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: ApikeyCreateBody,
) -> Response[Union[ApikeyCreateResponse200, Error]]:
    """Make an API key

     Scoped to `apps` and `methods`, read-only or not, for `ttlSecs`; `upgrades` makes an upgrade key,
    which signs upgrades of one app in place of a passkey; only an owner or admin of the app's
    organization makes one (`403 FORBIDDEN`), and its upgrades are judged by that rule again where they
    apply. `then` (at most 4, 64 KiB as JSON; not with `upgrades`) names confirm call methods of the
    key's own `apps` that the same passkey approves, each naming the new key: every string argument
    exactly `key:$key` is written `key:<keyId>` (a follow-up without one is `400 BAD_FOLLOW_UP`), the
    consent and the system log state each call's arguments only by hash (`argsHash`, SHA-256 of their
    JCS), the `412` details list them so (`then`), and the retry runs them in order before making the
    key; the first that fails answers `422 FOLLOW_UP_FAILED` (`details: { index, app, method, error,
    applied }`) and no key is made. A node before 1.0.0-rc.36 ignores `then` and makes a plain key: a
    client checks that the `412` lists `then` and that the answer's `then` is as long as its own. `412`
    first. The key is shown once.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        body (ApikeyCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ApikeyCreateResponse200, Error]]
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
    client: AuthenticatedClient,
    body: ApikeyCreateBody,
) -> Optional[Union[ApikeyCreateResponse200, Error]]:
    """Make an API key

     Scoped to `apps` and `methods`, read-only or not, for `ttlSecs`; `upgrades` makes an upgrade key,
    which signs upgrades of one app in place of a passkey; only an owner or admin of the app's
    organization makes one (`403 FORBIDDEN`), and its upgrades are judged by that rule again where they
    apply. `then` (at most 4, 64 KiB as JSON; not with `upgrades`) names confirm call methods of the
    key's own `apps` that the same passkey approves, each naming the new key: every string argument
    exactly `key:$key` is written `key:<keyId>` (a follow-up without one is `400 BAD_FOLLOW_UP`), the
    consent and the system log state each call's arguments only by hash (`argsHash`, SHA-256 of their
    JCS), the `412` details list them so (`then`), and the retry runs them in order before making the
    key; the first that fails answers `422 FOLLOW_UP_FAILED` (`details: { index, app, method, error,
    applied }`) and no key is made. A node before 1.0.0-rc.36 ignores `then` and makes a plain key: a
    client checks that the `412` lists `then` and that the answer's `then` is as long as its own. `412`
    first. The key is shown once.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        body (ApikeyCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ApikeyCreateResponse200, Error]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: ApikeyCreateBody,
) -> Response[Union[ApikeyCreateResponse200, Error]]:
    """Make an API key

     Scoped to `apps` and `methods`, read-only or not, for `ttlSecs`; `upgrades` makes an upgrade key,
    which signs upgrades of one app in place of a passkey; only an owner or admin of the app's
    organization makes one (`403 FORBIDDEN`), and its upgrades are judged by that rule again where they
    apply. `then` (at most 4, 64 KiB as JSON; not with `upgrades`) names confirm call methods of the
    key's own `apps` that the same passkey approves, each naming the new key: every string argument
    exactly `key:$key` is written `key:<keyId>` (a follow-up without one is `400 BAD_FOLLOW_UP`), the
    consent and the system log state each call's arguments only by hash (`argsHash`, SHA-256 of their
    JCS), the `412` details list them so (`then`), and the retry runs them in order before making the
    key; the first that fails answers `422 FOLLOW_UP_FAILED` (`details: { index, app, method, error,
    applied }`) and no key is made. A node before 1.0.0-rc.36 ignores `then` and makes a plain key: a
    client checks that the `412` lists `then` and that the answer's `then` is as long as its own. `412`
    first. The key is shown once.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        body (ApikeyCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ApikeyCreateResponse200, Error]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: ApikeyCreateBody,
) -> Optional[Union[ApikeyCreateResponse200, Error]]:
    """Make an API key

     Scoped to `apps` and `methods`, read-only or not, for `ttlSecs`; `upgrades` makes an upgrade key,
    which signs upgrades of one app in place of a passkey; only an owner or admin of the app's
    organization makes one (`403 FORBIDDEN`), and its upgrades are judged by that rule again where they
    apply. `then` (at most 4, 64 KiB as JSON; not with `upgrades`) names confirm call methods of the
    key's own `apps` that the same passkey approves, each naming the new key: every string argument
    exactly `key:$key` is written `key:<keyId>` (a follow-up without one is `400 BAD_FOLLOW_UP`), the
    consent and the system log state each call's arguments only by hash (`argsHash`, SHA-256 of their
    JCS), the `412` details list them so (`then`), and the retry runs them in order before making the
    key; the first that fails answers `422 FOLLOW_UP_FAILED` (`details: { index, app, method, error,
    applied }`) and no key is made. A node before 1.0.0-rc.36 ignores `then` and makes a plain key: a
    client checks that the `412` lists `then` and that the answer's `then` is as long as its own. `412`
    first. The key is shown once.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        body (ApikeyCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ApikeyCreateResponse200, Error]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
