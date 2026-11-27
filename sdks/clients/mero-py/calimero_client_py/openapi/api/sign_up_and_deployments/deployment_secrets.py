from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.deployment_secrets import DeploymentSecrets
from ...models.error import Error
from typing import cast


def _get_kwargs(
    id: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/deployments/{id}/secrets".format(
            id=id,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[DeploymentSecrets, Error]]:
    if response.status_code == 200:
        response_200 = DeploymentSecrets.from_dict(response.json())

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
) -> Response[Union[DeploymentSecrets, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[DeploymentSecrets, Error]]:
    """A deployment's secrets

     For its managers, on fresh state: every secret a release the deployment offers declares, or a
    release one of those references (`uses`, for the apps it creates), and every one it holds a value
    of. Each row names the deployment's apps whose release declares it or references one that does
    (`apps`, empty when none does any more: the value is kept, unused, until cleared), the releases that
    declare it (`declaredBy`) and `set`; a set one adds its `fingerprint` (the first 16 hex digits of
    the value's SHA-256), `setBy` (`account:<id>`) and `setAt` (Unix seconds). Never a value.

    **Who:** signed in (a session or an API key).

    Args:
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[DeploymentSecrets, Error]]
    """

    kwargs = _get_kwargs(
        id=id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[DeploymentSecrets, Error]]:
    """A deployment's secrets

     For its managers, on fresh state: every secret a release the deployment offers declares, or a
    release one of those references (`uses`, for the apps it creates), and every one it holds a value
    of. Each row names the deployment's apps whose release declares it or references one that does
    (`apps`, empty when none does any more: the value is kept, unused, until cleared), the releases that
    declare it (`declaredBy`) and `set`; a set one adds its `fingerprint` (the first 16 hex digits of
    the value's SHA-256), `setBy` (`account:<id>`) and `setAt` (Unix seconds). Never a value.

    **Who:** signed in (a session or an API key).

    Args:
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[DeploymentSecrets, Error]
    """

    return sync_detailed(
        id=id,
        client=client,
    ).parsed


async def asyncio_detailed(
    id: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[DeploymentSecrets, Error]]:
    """A deployment's secrets

     For its managers, on fresh state: every secret a release the deployment offers declares, or a
    release one of those references (`uses`, for the apps it creates), and every one it holds a value
    of. Each row names the deployment's apps whose release declares it or references one that does
    (`apps`, empty when none does any more: the value is kept, unused, until cleared), the releases that
    declare it (`declaredBy`) and `set`; a set one adds its `fingerprint` (the first 16 hex digits of
    the value's SHA-256), `setBy` (`account:<id>`) and `setAt` (Unix seconds). Never a value.

    **Who:** signed in (a session or an API key).

    Args:
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[DeploymentSecrets, Error]]
    """

    kwargs = _get_kwargs(
        id=id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[DeploymentSecrets, Error]]:
    """A deployment's secrets

     For its managers, on fresh state: every secret a release the deployment offers declares, or a
    release one of those references (`uses`, for the apps it creates), and every one it holds a value
    of. Each row names the deployment's apps whose release declares it or references one that does
    (`apps`, empty when none does any more: the value is kept, unused, until cleared), the releases that
    declare it (`declaredBy`) and `set`; a set one adds its `fingerprint` (the first 16 hex digits of
    the value's SHA-256), `setBy` (`account:<id>`) and `setAt` (Unix seconds). Never a value.

    **Who:** signed in (a session or an API key).

    Args:
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[DeploymentSecrets, Error]
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
        )
    ).parsed
