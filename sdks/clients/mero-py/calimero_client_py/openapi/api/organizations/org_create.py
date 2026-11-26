from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.org_body import OrgBody
from ...models.org_created import OrgCreated
from typing import cast


def _get_kwargs(
    *,
    body: OrgBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/orgs",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OrgCreated]]:
    if response.status_code == 200:
        response_200 = OrgCreated.from_dict(response.json())

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
) -> Response[Union[Error, OrgCreated]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: OrgBody,
) -> Response[Union[Error, OrgCreated]]:
    """Create an organization

     `recoveryKey` is the public half of an organization recovery key. `capabilities` (bits) is what
    members can do unless set for them, in place of the default; `409 FEATURE_NOT_ACTIVE` below cluster
    version 1.0.0-rc.29. From cluster version 1.0.0-rc.32 a sign-up makes no organization: an account's
    are those it creates here (or with a `standalone` instance) or is invited to; before, every account
    also had a personal organization from sign-up (`personal` in `/auth/me`).

    **Who:** a passkey session (an API key is refused).

    Args:
        body (OrgBody): The request body of `POST /orgs`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OrgCreated]]
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
    body: OrgBody,
) -> Optional[Union[Error, OrgCreated]]:
    """Create an organization

     `recoveryKey` is the public half of an organization recovery key. `capabilities` (bits) is what
    members can do unless set for them, in place of the default; `409 FEATURE_NOT_ACTIVE` below cluster
    version 1.0.0-rc.29. From cluster version 1.0.0-rc.32 a sign-up makes no organization: an account's
    are those it creates here (or with a `standalone` instance) or is invited to; before, every account
    also had a personal organization from sign-up (`personal` in `/auth/me`).

    **Who:** a passkey session (an API key is refused).

    Args:
        body (OrgBody): The request body of `POST /orgs`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OrgCreated]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: OrgBody,
) -> Response[Union[Error, OrgCreated]]:
    """Create an organization

     `recoveryKey` is the public half of an organization recovery key. `capabilities` (bits) is what
    members can do unless set for them, in place of the default; `409 FEATURE_NOT_ACTIVE` below cluster
    version 1.0.0-rc.29. From cluster version 1.0.0-rc.32 a sign-up makes no organization: an account's
    are those it creates here (or with a `standalone` instance) or is invited to; before, every account
    also had a personal organization from sign-up (`personal` in `/auth/me`).

    **Who:** a passkey session (an API key is refused).

    Args:
        body (OrgBody): The request body of `POST /orgs`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OrgCreated]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: OrgBody,
) -> Optional[Union[Error, OrgCreated]]:
    """Create an organization

     `recoveryKey` is the public half of an organization recovery key. `capabilities` (bits) is what
    members can do unless set for them, in place of the default; `409 FEATURE_NOT_ACTIVE` below cluster
    version 1.0.0-rc.29. From cluster version 1.0.0-rc.32 a sign-up makes no organization: an account's
    are those it creates here (or with a `standalone` instance) or is invited to; before, every account
    also had a personal organization from sign-up (`personal` in `/auth/me`).

    **Who:** a passkey session (an API key is refused).

    Args:
        body (OrgBody): The request body of `POST /orgs`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OrgCreated]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
