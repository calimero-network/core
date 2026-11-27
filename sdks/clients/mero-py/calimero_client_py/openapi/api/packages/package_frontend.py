from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.frontend_digests import FrontendDigests
from typing import cast


def _get_kwargs(
    package: str,
    version: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/packages/{package}/{version}/frontend".format(
            package=package,
            version=version,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, FrontendDigests]]:
    if response.status_code == 200:
        response_200 = FrontendDigests.from_dict(response.json())

        return response_200

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, FrontendDigests]]:
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
    client: Union[AuthenticatedClient, Client],
) -> Response[Union[Error, FrontendDigests]]:
    """A release's frontend files

     Every file the node serves for the release's frontend (at `/{version}/{path}` on the package's
    origin), with its SHA-256 and size, and the entry. The node verified each against the publisher's
    signature at install; a client that attested this node checks the page it was served against this
    list, so a TLS terminator or a certificate holder cannot swap the page unseen. Public, as the files
    are. `404` for a release that ships no frontend or that the cluster does not hold.

    **Who:** anyone.

    Args:
        package (str):
        version (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, FrontendDigests]]
    """

    kwargs = _get_kwargs(
        package=package,
        version=version,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    package: str,
    version: str,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[Union[Error, FrontendDigests]]:
    """A release's frontend files

     Every file the node serves for the release's frontend (at `/{version}/{path}` on the package's
    origin), with its SHA-256 and size, and the entry. The node verified each against the publisher's
    signature at install; a client that attested this node checks the page it was served against this
    list, so a TLS terminator or a certificate holder cannot swap the page unseen. Public, as the files
    are. `404` for a release that ships no frontend or that the cluster does not hold.

    **Who:** anyone.

    Args:
        package (str):
        version (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, FrontendDigests]
    """

    return sync_detailed(
        package=package,
        version=version,
        client=client,
    ).parsed


async def asyncio_detailed(
    package: str,
    version: str,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[Union[Error, FrontendDigests]]:
    """A release's frontend files

     Every file the node serves for the release's frontend (at `/{version}/{path}` on the package's
    origin), with its SHA-256 and size, and the entry. The node verified each against the publisher's
    signature at install; a client that attested this node checks the page it was served against this
    list, so a TLS terminator or a certificate holder cannot swap the page unseen. Public, as the files
    are. `404` for a release that ships no frontend or that the cluster does not hold.

    **Who:** anyone.

    Args:
        package (str):
        version (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, FrontendDigests]]
    """

    kwargs = _get_kwargs(
        package=package,
        version=version,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    package: str,
    version: str,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[Union[Error, FrontendDigests]]:
    """A release's frontend files

     Every file the node serves for the release's frontend (at `/{version}/{path}` on the package's
    origin), with its SHA-256 and size, and the entry. The node verified each against the publisher's
    signature at install; a client that attested this node checks the page it was served against this
    list, so a TLS terminator or a certificate holder cannot swap the page unseen. Public, as the files
    are. `404` for a release that ships no frontend or that the cluster does not hold.

    **Who:** anyone.

    Args:
        package (str):
        version (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, FrontendDigests]
    """

    return (
        await asyncio_detailed(
            package=package,
            version=version,
            client=client,
        )
    ).parsed
