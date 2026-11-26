from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.policy_view import PolicyView
from ...models.upgrade_policy_body import UpgradePolicyBody
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: UpgradePolicyBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/upgrade-policy".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PolicyView]]:
    if response.status_code == 200:
        response_200 = PolicyView.from_dict(response.json())

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

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, PolicyView]]:
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
    body: UpgradePolicyBody,
) -> Response[Union[Error, PolicyView]]:
    """Set an app's own update policy

     An owner or admin of the app's organization, with a step-up over `org:<org> / __set_upgrade_policy /
    { app, policy }`. `policy: null` (or left out) drops it: the app follows its organization's again.
    `holdHours` outside 24 to 168 is `400 BAD_UPGRADE_POLICY` before the step-up. A move to another
    organization drops it. `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (UpgradePolicyBody): A policy to set at one scope; `null` drops it (an app then
            follows its organization's, an organization the defaults).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PolicyView]]
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
    body: UpgradePolicyBody,
) -> Optional[Union[Error, PolicyView]]:
    """Set an app's own update policy

     An owner or admin of the app's organization, with a step-up over `org:<org> / __set_upgrade_policy /
    { app, policy }`. `policy: null` (or left out) drops it: the app follows its organization's again.
    `holdHours` outside 24 to 168 is `400 BAD_UPGRADE_POLICY` before the step-up. A move to another
    organization drops it. `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (UpgradePolicyBody): A policy to set at one scope; `null` drops it (an app then
            follows its organization's, an organization the defaults).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PolicyView]
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
    body: UpgradePolicyBody,
) -> Response[Union[Error, PolicyView]]:
    """Set an app's own update policy

     An owner or admin of the app's organization, with a step-up over `org:<org> / __set_upgrade_policy /
    { app, policy }`. `policy: null` (or left out) drops it: the app follows its organization's again.
    `holdHours` outside 24 to 168 is `400 BAD_UPGRADE_POLICY` before the step-up. A move to another
    organization drops it. `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (UpgradePolicyBody): A policy to set at one scope; `null` drops it (an app then
            follows its organization's, an organization the defaults).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PolicyView]]
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
    body: UpgradePolicyBody,
) -> Optional[Union[Error, PolicyView]]:
    """Set an app's own update policy

     An owner or admin of the app's organization, with a step-up over `org:<org> / __set_upgrade_policy /
    { app, policy }`. `policy: null` (or left out) drops it: the app follows its organization's again.
    `holdHours` outside 24 to 168 is `400 BAD_UPGRADE_POLICY` before the step-up. A move to another
    organization drops it. `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (UpgradePolicyBody): A policy to set at one scope; `null` drops it (an app then
            follows its organization's, an organization the defaults).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PolicyView]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
