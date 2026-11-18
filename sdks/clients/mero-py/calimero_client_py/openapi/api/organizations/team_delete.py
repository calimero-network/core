from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.confirm_only import ConfirmOnly
from ...models.error import Error
from ...models.team_delete_response_200 import TeamDeleteResponse200
from typing import cast


def _get_kwargs(
    org: str,
    team: str,
    *,
    body: ConfirmOnly,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/orgs/{org}/teams/{team}".format(
            org=org,
            team=team,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, TeamDeleteResponse200]]:
    if response.status_code == 200:
        response_200 = TeamDeleteResponse200.from_dict(response.json())

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
) -> Response[Union[Error, TeamDeleteResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    org: str,
    team: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Response[Union[Error, TeamDeleteResponse200]]:
    """Delete a team

     With the teams and apps below it. A package-bound session: only its own package's teams (its app
    team and the teams below it): a team below its app team whose apps are all its package's.

    **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED`
    first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        org (str):
        team (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, TeamDeleteResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        team=team,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    org: str,
    team: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Optional[Union[Error, TeamDeleteResponse200]]:
    """Delete a team

     With the teams and apps below it. A package-bound session: only its own package's teams (its app
    team and the teams below it): a team below its app team whose apps are all its package's.

    **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED`
    first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        org (str):
        team (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, TeamDeleteResponse200]
    """

    return sync_detailed(
        org=org,
        team=team,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    org: str,
    team: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Response[Union[Error, TeamDeleteResponse200]]:
    """Delete a team

     With the teams and apps below it. A package-bound session: only its own package's teams (its app
    team and the teams below it): a team below its app team whose apps are all its package's.

    **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED`
    first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        org (str):
        team (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, TeamDeleteResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        team=team,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    org: str,
    team: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Optional[Union[Error, TeamDeleteResponse200]]:
    """Delete a team

     With the teams and apps below it. A package-bound session: only its own package's teams (its app
    team and the teams below it): a team below its app team whose apps are all its package's.

    **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED`
    first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        org (str):
        team (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, TeamDeleteResponse200]
    """

    return (
        await asyncio_detailed(
            org=org,
            team=team,
            client=client,
            body=body,
        )
    ).parsed
