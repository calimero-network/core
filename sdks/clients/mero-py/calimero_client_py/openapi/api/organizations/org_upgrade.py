from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.org_upgrade_body import OrgUpgradeBody
from ...models.upgrade_run import UpgradeRun
from typing import cast


def _get_kwargs(
    org: str,
    *,
    body: OrgUpgradeBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/orgs/{org}/upgrade".format(
            org=org,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, UpgradeRun]]:
    if response.status_code == 200:
        response_200 = UpgradeRun.from_dict(response.json())

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
) -> Response[Union[Error, UpgradeRun]]:
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
    body: OrgUpgradeBody,
) -> Response[Union[Error, UpgradeRun]]:
    """Start an upgrade run

     Upgrade every app of the organization (or of `team`, with `cascade` its subtree) that runs `package`
    to `version`, with one passkey assertion over the whole run; whoever holds `MANAGE_APPLICATION`
    there. Each app is upgraded as `POST /apps/{app}/upgrade` would; `GET /orgs/{org}/upgrades/{run}`
    follows it.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UpgradeRun]]
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
    body: OrgUpgradeBody,
) -> Optional[Union[Error, UpgradeRun]]:
    """Start an upgrade run

     Upgrade every app of the organization (or of `team`, with `cascade` its subtree) that runs `package`
    to `version`, with one passkey assertion over the whole run; whoever holds `MANAGE_APPLICATION`
    there. Each app is upgraded as `POST /apps/{app}/upgrade` would; `GET /orgs/{org}/upgrades/{run}`
    follows it.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UpgradeRun]
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
    body: OrgUpgradeBody,
) -> Response[Union[Error, UpgradeRun]]:
    """Start an upgrade run

     Upgrade every app of the organization (or of `team`, with `cascade` its subtree) that runs `package`
    to `version`, with one passkey assertion over the whole run; whoever holds `MANAGE_APPLICATION`
    there. Each app is upgraded as `POST /apps/{app}/upgrade` would; `GET /orgs/{org}/upgrades/{run}`
    follows it.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UpgradeRun]]
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
    body: OrgUpgradeBody,
) -> Optional[Union[Error, UpgradeRun]]:
    """Start an upgrade run

     Upgrade every app of the organization (or of `team`, with `cascade` its subtree) that runs `package`
    to `version`, with one passkey assertion over the whole run; whoever holds `MANAGE_APPLICATION`
    there. Each app is upgraded as `POST /apps/{app}/upgrade` would; `GET /orgs/{org}/upgrades/{run}`
    follows it.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UpgradeRun]
    """

    return (
        await asyncio_detailed(
            org=org,
            client=client,
            body=body,
        )
    ).parsed
