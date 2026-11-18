from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.team_create_body import TeamCreateBody
from ...models.team_create_response_200 import TeamCreateResponse200
from typing import cast


def _get_kwargs(
    org: str,
    *,
    body: TeamCreateBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/orgs/{org}/teams".format(
            org=org,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, TeamCreateResponse200]]:
    if response.status_code == 200:
        response_200 = TeamCreateResponse200.from_dict(response.json())

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
) -> Response[Union[Error, TeamCreateResponse200]]:
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
    body: TeamCreateBody,
) -> Response[Union[Error, TeamCreateResponse200]]:
    """Create a team

     An admin, a lead of `parent`, a member with `CREATE_TEAM` at the top, or a member who holds
    `CREATE_CHILD_TEAM` under `parent` (a restricted team only). A package-bound session: only its own
    package's teams (its app team and the teams below it): `parent` one of them.

    **Who:** a passkey session (an API key is refused).

    Args:
        org (str):
        body (TeamCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, TeamCreateResponse200]]
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
    body: TeamCreateBody,
) -> Optional[Union[Error, TeamCreateResponse200]]:
    """Create a team

     An admin, a lead of `parent`, a member with `CREATE_TEAM` at the top, or a member who holds
    `CREATE_CHILD_TEAM` under `parent` (a restricted team only). A package-bound session: only its own
    package's teams (its app team and the teams below it): `parent` one of them.

    **Who:** a passkey session (an API key is refused).

    Args:
        org (str):
        body (TeamCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, TeamCreateResponse200]
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
    body: TeamCreateBody,
) -> Response[Union[Error, TeamCreateResponse200]]:
    """Create a team

     An admin, a lead of `parent`, a member with `CREATE_TEAM` at the top, or a member who holds
    `CREATE_CHILD_TEAM` under `parent` (a restricted team only). A package-bound session: only its own
    package's teams (its app team and the teams below it): `parent` one of them.

    **Who:** a passkey session (an API key is refused).

    Args:
        org (str):
        body (TeamCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, TeamCreateResponse200]]
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
    body: TeamCreateBody,
) -> Optional[Union[Error, TeamCreateResponse200]]:
    """Create a team

     An admin, a lead of `parent`, a member with `CREATE_TEAM` at the top, or a member who holds
    `CREATE_CHILD_TEAM` under `parent` (a restricted team only). A package-bound session: only its own
    package's teams (its app team and the teams below it): `parent` one of them.

    **Who:** a passkey session (an API key is refused).

    Args:
        org (str):
        body (TeamCreateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, TeamCreateResponse200]
    """

    return (
        await asyncio_detailed(
            org=org,
            client=client,
            body=body,
        )
    ).parsed
