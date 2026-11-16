from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.member_remove_teams_body import MemberRemoveTeamsBody
from ...models.member_remove_teams_response_200 import MemberRemoveTeamsResponse200
from typing import cast


def _get_kwargs(
    org: str,
    account: str,
    *,
    body: MemberRemoveTeamsBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/orgs/{org}/members/{account}/teams".format(
            org=org,
            account=account,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, MemberRemoveTeamsResponse200]]:
    if response.status_code == 200:
        response_200 = MemberRemoveTeamsResponse200.from_dict(response.json())

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

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, MemberRemoveTeamsResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    org: str,
    account: str,
    *,
    client: AuthenticatedClient,
    body: MemberRemoveTeamsBody,
) -> Response[Union[Error, MemberRemoveTeamsResponse200]]:
    """Remove a member from several teams, or leave them

     Each judged as `DELETE /orgs/{org}/teams/{team}/members/{account}` would judge it, and none taken
    unless every one may be. 1 to 64 distinct teams; one passkey, `412` first.

    **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED`
    first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        org (str):
        account (str):
        body (MemberRemoveTeamsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, MemberRemoveTeamsResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        account=account,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    org: str,
    account: str,
    *,
    client: AuthenticatedClient,
    body: MemberRemoveTeamsBody,
) -> Optional[Union[Error, MemberRemoveTeamsResponse200]]:
    """Remove a member from several teams, or leave them

     Each judged as `DELETE /orgs/{org}/teams/{team}/members/{account}` would judge it, and none taken
    unless every one may be. 1 to 64 distinct teams; one passkey, `412` first.

    **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED`
    first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        org (str):
        account (str):
        body (MemberRemoveTeamsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, MemberRemoveTeamsResponse200]
    """

    return sync_detailed(
        org=org,
        account=account,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    org: str,
    account: str,
    *,
    client: AuthenticatedClient,
    body: MemberRemoveTeamsBody,
) -> Response[Union[Error, MemberRemoveTeamsResponse200]]:
    """Remove a member from several teams, or leave them

     Each judged as `DELETE /orgs/{org}/teams/{team}/members/{account}` would judge it, and none taken
    unless every one may be. 1 to 64 distinct teams; one passkey, `412` first.

    **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED`
    first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        org (str):
        account (str):
        body (MemberRemoveTeamsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, MemberRemoveTeamsResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        account=account,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    org: str,
    account: str,
    *,
    client: AuthenticatedClient,
    body: MemberRemoveTeamsBody,
) -> Optional[Union[Error, MemberRemoveTeamsResponse200]]:
    """Remove a member from several teams, or leave them

     Each judged as `DELETE /orgs/{org}/teams/{team}/members/{account}` would judge it, and none taken
    unless every one may be. 1 to 64 distinct teams; one passkey, `412` first.

    **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED`
    first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        org (str):
        account (str):
        body (MemberRemoveTeamsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, MemberRemoveTeamsResponse200]
    """

    return (
        await asyncio_detailed(
            org=org,
            account=account,
            client=client,
            body=body,
        )
    ).parsed
