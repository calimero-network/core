from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.public_links import PublicLinks
from typing import cast


def _get_kwargs(
    app: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/public-links".format(
            app=app,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PublicLinks]]:
    if response.status_code == 200:
        response_200 = PublicLinks.from_dict(response.json())

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

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, PublicLinks]]:
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
) -> Response[Union[Error, PublicLinks]]:
    """An instance's public links

     For whoever may change its access: each link's key, terms and creator, never a token.

    **Who:** a passkey session (an API key is refused).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PublicLinks]]
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
) -> Optional[Union[Error, PublicLinks]]:
    """An instance's public links

     For whoever may change its access: each link's key, terms and creator, never a token.

    **Who:** a passkey session (an API key is refused).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PublicLinks]
    """

    return sync_detailed(
        app=app,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, PublicLinks]]:
    """An instance's public links

     For whoever may change its access: each link's key, terms and creator, never a token.

    **Who:** a passkey session (an API key is refused).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PublicLinks]]
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
) -> Optional[Union[Error, PublicLinks]]:
    """An instance's public links

     For whoever may change its access: each link's key, terms and creator, never a token.

    **Who:** a passkey session (an API key is refused).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PublicLinks]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
        )
    ).parsed
