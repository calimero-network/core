from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.package_capabilities_response_200 import PackageCapabilitiesResponse200
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    package: str,
    version: str,
    *,
    service: Union[Unset, str] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["service"] = service

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/packages/{package}/{version}/capabilities".format(
            package=package,
            version=version,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PackageCapabilitiesResponse200]]:
    if response.status_code == 200:
        response_200 = PackageCapabilitiesResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

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
) -> Response[Union[Error, PackageCapabilitiesResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    package: str,
    version: str,
    *,
    client: AuthenticatedClient,
    service: Union[Unset, str] = UNSET,
) -> Response[Union[Error, PackageCapabilitiesResponse200]]:
    """A release's capabilities

     What the release is and may do, read from its signed manifest and the ABI its service component
    embeds, before any app of it exists: `bundle` is the service WASM's hash, `outbound` the sorted
    prefixes it may call out to and `abi` the embedded `calimero:abi` verbatim (`null` when it embeds
    none). `service` names the service whose card to read (default: the release's only one). A session
    bound to the package reads every release of it. `404` for a release the caller may not see, `400
    NO_SUCH_SERVICE` for an unknown service or no `service` on a release with several, `400 BAD_REQUEST`
    for an over-long `service`.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):
        version (str):
        service (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PackageCapabilitiesResponse200]]
    """

    kwargs = _get_kwargs(
        package=package,
        version=version,
        service=service,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    package: str,
    version: str,
    *,
    client: AuthenticatedClient,
    service: Union[Unset, str] = UNSET,
) -> Optional[Union[Error, PackageCapabilitiesResponse200]]:
    """A release's capabilities

     What the release is and may do, read from its signed manifest and the ABI its service component
    embeds, before any app of it exists: `bundle` is the service WASM's hash, `outbound` the sorted
    prefixes it may call out to and `abi` the embedded `calimero:abi` verbatim (`null` when it embeds
    none). `service` names the service whose card to read (default: the release's only one). A session
    bound to the package reads every release of it. `404` for a release the caller may not see, `400
    NO_SUCH_SERVICE` for an unknown service or no `service` on a release with several, `400 BAD_REQUEST`
    for an over-long `service`.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):
        version (str):
        service (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PackageCapabilitiesResponse200]
    """

    return sync_detailed(
        package=package,
        version=version,
        client=client,
        service=service,
    ).parsed


async def asyncio_detailed(
    package: str,
    version: str,
    *,
    client: AuthenticatedClient,
    service: Union[Unset, str] = UNSET,
) -> Response[Union[Error, PackageCapabilitiesResponse200]]:
    """A release's capabilities

     What the release is and may do, read from its signed manifest and the ABI its service component
    embeds, before any app of it exists: `bundle` is the service WASM's hash, `outbound` the sorted
    prefixes it may call out to and `abi` the embedded `calimero:abi` verbatim (`null` when it embeds
    none). `service` names the service whose card to read (default: the release's only one). A session
    bound to the package reads every release of it. `404` for a release the caller may not see, `400
    NO_SUCH_SERVICE` for an unknown service or no `service` on a release with several, `400 BAD_REQUEST`
    for an over-long `service`.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):
        version (str):
        service (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PackageCapabilitiesResponse200]]
    """

    kwargs = _get_kwargs(
        package=package,
        version=version,
        service=service,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    package: str,
    version: str,
    *,
    client: AuthenticatedClient,
    service: Union[Unset, str] = UNSET,
) -> Optional[Union[Error, PackageCapabilitiesResponse200]]:
    """A release's capabilities

     What the release is and may do, read from its signed manifest and the ABI its service component
    embeds, before any app of it exists: `bundle` is the service WASM's hash, `outbound` the sorted
    prefixes it may call out to and `abi` the embedded `calimero:abi` verbatim (`null` when it embeds
    none). `service` names the service whose card to read (default: the release's only one). A session
    bound to the package reads every release of it. `404` for a release the caller may not see, `400
    NO_SUCH_SERVICE` for an unknown service or no `service` on a release with several, `400 BAD_REQUEST`
    for an over-long `service`.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):
        version (str):
        service (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PackageCapabilitiesResponse200]
    """

    return (
        await asyncio_detailed(
            package=package,
            version=version,
            client=client,
            service=service,
        )
    ).parsed
