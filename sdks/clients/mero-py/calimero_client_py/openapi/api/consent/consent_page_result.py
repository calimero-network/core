from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.consent_page_result_response_200 import ConsentPageResultResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    request: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/consent/{request}/result".format(
            request=request,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ConsentPageResultResponse200, Error]]:
    if response.status_code == 200:
        response_200 = ConsentPageResultResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 503:
        response_503 = Error.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ConsentPageResultResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    request: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[ConsentPageResultResponse200, Error]]:
    """What became of a consent request

     `decidedAt` (Unix seconds) is `null` while it is pending. `outcome`, once approved, is what the
    approval made: `{ orgId }` for `createOrg` (with `teamId`, the asking app's own team the
    organization starts with, when an app's frontend asked on a cluster that makes it), `{ token }` for
    `createInvite`, `{ org }` for `acceptInvite`, `{ orgId, name }` for `renameOrg`, `{ apps }` (the
    apps deleted) for `deleteOrg`; absent for the other kinds.

    **Who:** a passkey session (an API key is refused).

    Args:
        request (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ConsentPageResultResponse200, Error]]
    """

    kwargs = _get_kwargs(
        request=request,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    request: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[ConsentPageResultResponse200, Error]]:
    """What became of a consent request

     `decidedAt` (Unix seconds) is `null` while it is pending. `outcome`, once approved, is what the
    approval made: `{ orgId }` for `createOrg` (with `teamId`, the asking app's own team the
    organization starts with, when an app's frontend asked on a cluster that makes it), `{ token }` for
    `createInvite`, `{ org }` for `acceptInvite`, `{ orgId, name }` for `renameOrg`, `{ apps }` (the
    apps deleted) for `deleteOrg`; absent for the other kinds.

    **Who:** a passkey session (an API key is refused).

    Args:
        request (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ConsentPageResultResponse200, Error]
    """

    return sync_detailed(
        request=request,
        client=client,
    ).parsed


async def asyncio_detailed(
    request: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[ConsentPageResultResponse200, Error]]:
    """What became of a consent request

     `decidedAt` (Unix seconds) is `null` while it is pending. `outcome`, once approved, is what the
    approval made: `{ orgId }` for `createOrg` (with `teamId`, the asking app's own team the
    organization starts with, when an app's frontend asked on a cluster that makes it), `{ token }` for
    `createInvite`, `{ org }` for `acceptInvite`, `{ orgId, name }` for `renameOrg`, `{ apps }` (the
    apps deleted) for `deleteOrg`; absent for the other kinds.

    **Who:** a passkey session (an API key is refused).

    Args:
        request (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ConsentPageResultResponse200, Error]]
    """

    kwargs = _get_kwargs(
        request=request,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    request: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[ConsentPageResultResponse200, Error]]:
    """What became of a consent request

     `decidedAt` (Unix seconds) is `null` while it is pending. `outcome`, once approved, is what the
    approval made: `{ orgId }` for `createOrg` (with `teamId`, the asking app's own team the
    organization starts with, when an app's frontend asked on a cluster that makes it), `{ token }` for
    `createInvite`, `{ org }` for `acceptInvite`, `{ orgId, name }` for `renameOrg`, `{ apps }` (the
    apps deleted) for `deleteOrg`; absent for the other kinds.

    **Who:** a passkey session (an API key is refused).

    Args:
        request (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ConsentPageResultResponse200, Error]
    """

    return (
        await asyncio_detailed(
            request=request,
            client=client,
        )
    ).parsed
