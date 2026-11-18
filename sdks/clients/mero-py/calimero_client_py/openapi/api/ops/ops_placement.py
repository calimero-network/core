from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.ops_placement_response_200 import OpsPlacementResponse200
from typing import cast


def _get_kwargs() -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/ops/placement",
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OpsPlacementResponse200]]:
    if response.status_code == 200:
        response_200 = OpsPlacementResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, OpsPlacementResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, OpsPlacementResponse200]]:
    """Where every app's replicas are

     Ids and counts only, read after a read index (`fresh`: this node caught up with the system leader);
    `release` is a node's attested release; `clusterVersion` is the logged cluster version (`null` until
    the system leader logs one).

    **Who:** the ops token.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsPlacementResponse200]]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, OpsPlacementResponse200]]:
    """Where every app's replicas are

     Ids and counts only, read after a read index (`fresh`: this node caught up with the system leader);
    `release` is a node's attested release; `clusterVersion` is the logged cluster version (`null` until
    the system leader logs one).

    **Who:** the ops token.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsPlacementResponse200]
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, OpsPlacementResponse200]]:
    """Where every app's replicas are

     Ids and counts only, read after a read index (`fresh`: this node caught up with the system leader);
    `release` is a node's attested release; `clusterVersion` is the logged cluster version (`null` until
    the system leader logs one).

    **Who:** the ops token.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsPlacementResponse200]]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, OpsPlacementResponse200]]:
    """Where every app's replicas are

     Ids and counts only, read after a read index (`fresh`: this node caught up with the system leader);
    `release` is a node's attested release; `clusterVersion` is the logged cluster version (`null` until
    the system leader logs one).

    **Who:** the ops token.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsPlacementResponse200]
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
