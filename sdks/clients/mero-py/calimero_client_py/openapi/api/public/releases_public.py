from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.cluster_releases import ClusterReleases
from typing import cast


def _get_kwargs() -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/releases",
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[ClusterReleases]:
    if response.status_code == 200:
        response_200 = ClusterReleases.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[ClusterReleases]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[ClusterReleases]:
    """The cluster's releases

     The cluster's governance and the deadlines set on mero-tee releases, oldest release first: after
    `after` (RFC 3339; `afterSecs` Unix seconds) a scope that has not approved that release (or a newer
    one) starts no new work.

    **Who:** anyone.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ClusterReleases]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[ClusterReleases]:
    """The cluster's releases

     The cluster's governance and the deadlines set on mero-tee releases, oldest release first: after
    `after` (RFC 3339; `afterSecs` Unix seconds) a scope that has not approved that release (or a newer
    one) starts no new work.

    **Who:** anyone.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ClusterReleases
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[ClusterReleases]:
    """The cluster's releases

     The cluster's governance and the deadlines set on mero-tee releases, oldest release first: after
    `after` (RFC 3339; `afterSecs` Unix seconds) a scope that has not approved that release (or a newer
    one) starts no new work.

    **Who:** anyone.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ClusterReleases]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[ClusterReleases]:
    """The cluster's releases

     The cluster's governance and the deadlines set on mero-tee releases, oldest release first: after
    `after` (RFC 3339; `afterSecs` Unix seconds) a scope that has not approved that release (or a newer
    one) starts no new work.

    **Who:** anyone.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ClusterReleases
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
