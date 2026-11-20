from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_links_response_200 import AppLinksResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/links".format(
            app=app,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppLinksResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppLinksResponse200.from_dict(response.json())

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
) -> Response[Union[AppLinksResponse200, Error]]:
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
) -> Response[Union[AppLinksResponse200, Error]]:
    """List an app's links to the apps its release uses

     One row per reference (`uses`) of the release the app runs, with the app linked under it: a member
    made with the instance (`mode: create`) or an app its owner attached (`mode: attach`), `null` when
    none. `version` is the release that app runs and `inRange` whether it is inside the reference's
    range (`null` with nothing linked). A link the release no longer names is listed with `package`,
    `range` and `mode` `null`.

    **Who:** whoever the app's access admits (`anyone`: any signed-in account; the legacy `public`:
    signed out too).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppLinksResponse200, Error]]
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
) -> Optional[Union[AppLinksResponse200, Error]]:
    """List an app's links to the apps its release uses

     One row per reference (`uses`) of the release the app runs, with the app linked under it: a member
    made with the instance (`mode: create`) or an app its owner attached (`mode: attach`), `null` when
    none. `version` is the release that app runs and `inRange` whether it is inside the reference's
    range (`null` with nothing linked). A link the release no longer names is listed with `package`,
    `range` and `mode` `null`.

    **Who:** whoever the app's access admits (`anyone`: any signed-in account; the legacy `public`:
    signed out too).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppLinksResponse200, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppLinksResponse200, Error]]:
    """List an app's links to the apps its release uses

     One row per reference (`uses`) of the release the app runs, with the app linked under it: a member
    made with the instance (`mode: create`) or an app its owner attached (`mode: attach`), `null` when
    none. `version` is the release that app runs and `inRange` whether it is inside the reference's
    range (`null` with nothing linked). A link the release no longer names is listed with `package`,
    `range` and `mode` `null`.

    **Who:** whoever the app's access admits (`anyone`: any signed-in account; the legacy `public`:
    signed out too).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppLinksResponse200, Error]]
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
) -> Optional[Union[AppLinksResponse200, Error]]:
    """List an app's links to the apps its release uses

     One row per reference (`uses`) of the release the app runs, with the app linked under it: a member
    made with the instance (`mode: create`) or an app its owner attached (`mode: attach`), `null` when
    none. `version` is the release that app runs and `inRange` whether it is inside the reference's
    range (`null` with nothing linked). A link the release no longer names is listed with `package`,
    `range` and `mode` `null`.

    **Who:** whoever the app's access admits (`anyone`: any signed-in account; the legacy `public`:
    signed out too).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppLinksResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
        )
    ).parsed
