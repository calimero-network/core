from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.org_egress_body import OrgEgressBody
from ...models.org_egress_response_200 import OrgEgressResponse200
from typing import cast


def _get_kwargs(
    org: str,
    *,
    body: OrgEgressBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/orgs/{org}/egress".format(
            org=org,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OrgEgressResponse200]]:
    if response.status_code == 200:
        response_200 = OrgEgressResponse200.from_dict(response.json())

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
) -> Response[Union[Error, OrgEgressResponse200]]:
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
    body: OrgEgressBody,
) -> Response[Union[Error, OrgEgressResponse200]]:
    """Turn the organization's egress off or on

     `egress: false` turns egress off for every app of the organization: from each app's next outbound
    request on, nothing leaves the cluster for it, whatever its release declares and whatever an
    approval (the organization's or the operator's) says, and the organization approves no new outbound
    host (`409 EGRESS_OFF`). `egress: true` turns it back on, a red change made only on the consent page
    (`kind: setEgress`); from any other client `403 CONSENT_PAGE_REQUIRED`. The owner or an admin, with
    a passkey (`412` first); `403` to a member below admin, `404` to anyone outside it.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgEgressBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OrgEgressResponse200]]
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
    body: OrgEgressBody,
) -> Optional[Union[Error, OrgEgressResponse200]]:
    """Turn the organization's egress off or on

     `egress: false` turns egress off for every app of the organization: from each app's next outbound
    request on, nothing leaves the cluster for it, whatever its release declares and whatever an
    approval (the organization's or the operator's) says, and the organization approves no new outbound
    host (`409 EGRESS_OFF`). `egress: true` turns it back on, a red change made only on the consent page
    (`kind: setEgress`); from any other client `403 CONSENT_PAGE_REQUIRED`. The owner or an admin, with
    a passkey (`412` first); `403` to a member below admin, `404` to anyone outside it.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgEgressBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OrgEgressResponse200]
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
    body: OrgEgressBody,
) -> Response[Union[Error, OrgEgressResponse200]]:
    """Turn the organization's egress off or on

     `egress: false` turns egress off for every app of the organization: from each app's next outbound
    request on, nothing leaves the cluster for it, whatever its release declares and whatever an
    approval (the organization's or the operator's) says, and the organization approves no new outbound
    host (`409 EGRESS_OFF`). `egress: true` turns it back on, a red change made only on the consent page
    (`kind: setEgress`); from any other client `403 CONSENT_PAGE_REQUIRED`. The owner or an admin, with
    a passkey (`412` first); `403` to a member below admin, `404` to anyone outside it.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgEgressBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OrgEgressResponse200]]
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
    body: OrgEgressBody,
) -> Optional[Union[Error, OrgEgressResponse200]]:
    """Turn the organization's egress off or on

     `egress: false` turns egress off for every app of the organization: from each app's next outbound
    request on, nothing leaves the cluster for it, whatever its release declares and whatever an
    approval (the organization's or the operator's) says, and the organization approves no new outbound
    host (`409 EGRESS_OFF`). `egress: true` turns it back on, a red change made only on the consent page
    (`kind: setEgress`); from any other client `403 CONSENT_PAGE_REQUIRED`. The owner or an admin, with
    a passkey (`412` first); `403` to a member below admin, `404` to anyone outside it.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgEgressBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OrgEgressResponse200]
    """

    return (
        await asyncio_detailed(
            org=org,
            client=client,
            body=body,
        )
    ).parsed
