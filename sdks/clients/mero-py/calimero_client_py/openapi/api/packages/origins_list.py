from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.origins_list_response_200 import OriginsListResponse200
from typing import cast


def _get_kwargs(
    package: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/package-origins/{package}".format(
            package=package,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OriginsListResponse200]]:
    if response.status_code == 200:
        response_200 = OriginsListResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, OriginsListResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    package: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, OriginsListResponse200]]:
    """A package's frontend origins

     The custom origins (and rp-ids) the package's manager registered for its frontend.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OriginsListResponse200]]
    """

    kwargs = _get_kwargs(
        package=package,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    package: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, OriginsListResponse200]]:
    """A package's frontend origins

     The custom origins (and rp-ids) the package's manager registered for its frontend.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OriginsListResponse200]
    """

    return sync_detailed(
        package=package,
        client=client,
    ).parsed


async def asyncio_detailed(
    package: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, OriginsListResponse200]]:
    """A package's frontend origins

     The custom origins (and rp-ids) the package's manager registered for its frontend.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OriginsListResponse200]]
    """

    kwargs = _get_kwargs(
        package=package,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    package: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, OriginsListResponse200]]:
    """A package's frontend origins

     The custom origins (and rp-ids) the package's manager registered for its frontend.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OriginsListResponse200]
    """

    return (
        await asyncio_detailed(
            package=package,
            client=client,
        )
    ).parsed
