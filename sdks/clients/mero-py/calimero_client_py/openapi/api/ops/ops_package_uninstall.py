from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.uninstalled import Uninstalled
from typing import cast


def _get_kwargs(
    package: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/ops/packages/{package}".format(
            package=package,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, Uninstalled]]:
    if response.status_code == 200:
        response_200 = Uninstalled.from_dict(response.json())

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
) -> Response[Union[Error, Uninstalled]]:
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
) -> Response[Union[Error, Uninstalled]]:
    """Uninstall what the operator installed

     **Who:** the ops token.

    Args:
        package (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, Uninstalled]]
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
) -> Optional[Union[Error, Uninstalled]]:
    """Uninstall what the operator installed

     **Who:** the ops token.

    Args:
        package (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, Uninstalled]
    """

    return sync_detailed(
        package=package,
        client=client,
    ).parsed


async def asyncio_detailed(
    package: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, Uninstalled]]:
    """Uninstall what the operator installed

     **Who:** the ops token.

    Args:
        package (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, Uninstalled]]
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
) -> Optional[Union[Error, Uninstalled]]:
    """Uninstall what the operator installed

     **Who:** the ops token.

    Args:
        package (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, Uninstalled]
    """

    return (
        await asyncio_detailed(
            package=package,
            client=client,
        )
    ).parsed
