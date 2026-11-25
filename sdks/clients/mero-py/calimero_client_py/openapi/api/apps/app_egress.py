from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_egress_response_200 import AppEgressResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/egress".format(
            app=app,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppEgressResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppEgressResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AppEgressResponse200, Error]]:
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
) -> Response[Union[AppEgressResponse200, Error]]:
    """Where an app's unit may send data

     For whoever may read the app (`GET /apps/{app}`'s rule). `destinations`: every destination
    (`outbound` prefix, and a declared secret's host no prefix names) of the release the instance runs
    and of each app it created with it (`links`, transitively; never an attached app), sorted; empty
    when no data leaves Calimero. An external endpoint, a TEE included, is listed like any other: the
    platform never vouches for one (an app that trusts one verifies it in its own code). `members`: each
    member by its path (`<name>`, a member's own `<name>/<name>`) with its app id, release, signer and
    own destinations. A development bundle declares none. `egress`: `off` while its organization
    (`offBy: organization`) or the instance (`instance`) turned egress off (`offBy` and `summary` are
    absent while it is on), when none of the destinations is reached and `summary` says the app cannot
    send data outside the cluster.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppEgressResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppEgressResponse200, Error]]:
    """Where an app's unit may send data

     For whoever may read the app (`GET /apps/{app}`'s rule). `destinations`: every destination
    (`outbound` prefix, and a declared secret's host no prefix names) of the release the instance runs
    and of each app it created with it (`links`, transitively; never an attached app), sorted; empty
    when no data leaves Calimero. An external endpoint, a TEE included, is listed like any other: the
    platform never vouches for one (an app that trusts one verifies it in its own code). `members`: each
    member by its path (`<name>`, a member's own `<name>/<name>`) with its app id, release, signer and
    own destinations. A development bundle declares none. `egress`: `off` while its organization
    (`offBy: organization`) or the instance (`instance`) turned egress off (`offBy` and `summary` are
    absent while it is on), when none of the destinations is reached and `summary` says the app cannot
    send data outside the cluster.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppEgressResponse200, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppEgressResponse200, Error]]:
    """Where an app's unit may send data

     For whoever may read the app (`GET /apps/{app}`'s rule). `destinations`: every destination
    (`outbound` prefix, and a declared secret's host no prefix names) of the release the instance runs
    and of each app it created with it (`links`, transitively; never an attached app), sorted; empty
    when no data leaves Calimero. An external endpoint, a TEE included, is listed like any other: the
    platform never vouches for one (an app that trusts one verifies it in its own code). `members`: each
    member by its path (`<name>`, a member's own `<name>/<name>`) with its app id, release, signer and
    own destinations. A development bundle declares none. `egress`: `off` while its organization
    (`offBy: organization`) or the instance (`instance`) turned egress off (`offBy` and `summary` are
    absent while it is on), when none of the destinations is reached and `summary` says the app cannot
    send data outside the cluster.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppEgressResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppEgressResponse200, Error]]:
    """Where an app's unit may send data

     For whoever may read the app (`GET /apps/{app}`'s rule). `destinations`: every destination
    (`outbound` prefix, and a declared secret's host no prefix names) of the release the instance runs
    and of each app it created with it (`links`, transitively; never an attached app), sorted; empty
    when no data leaves Calimero. An external endpoint, a TEE included, is listed like any other: the
    platform never vouches for one (an app that trusts one verifies it in its own code). `members`: each
    member by its path (`<name>`, a member's own `<name>/<name>`) with its app id, release, signer and
    own destinations. A development bundle declares none. `egress`: `off` while its organization
    (`offBy: organization`) or the instance (`instance`) turned egress off (`offBy` and `summary` are
    absent while it is on), when none of the destinations is reached and `summary` says the app cannot
    send data outside the cluster.

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppEgressResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
        )
    ).parsed
