from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.member_metadata_body import MemberMetadataBody
from ...models.member_metadata_response_200 import MemberMetadataResponse200
from typing import cast


def _get_kwargs(
    org: str,
    account: str,
    *,
    body: MemberMetadataBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/orgs/{org}/members/{account}/metadata".format(
            org=org,
            account=account,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, MemberMetadataResponse200]]:
    if response.status_code == 200:
        response_200 = MemberMetadataResponse200.from_dict(response.json())

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
) -> Response[Union[Error, MemberMetadataResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    org: str,
    account: str,
    *,
    client: AuthenticatedClient,
    body: MemberMetadataBody,
) -> Response[Union[Error, MemberMetadataResponse200]]:
    """Set a member's metadata

     `metadata` is the caller's own JSON, free-form, kept as given.

    **Who:** signed in (a session or an API key).

    Args:
        org (str):
        account (str):
        body (MemberMetadataBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, MemberMetadataResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        account=account,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    org: str,
    account: str,
    *,
    client: AuthenticatedClient,
    body: MemberMetadataBody,
) -> Optional[Union[Error, MemberMetadataResponse200]]:
    """Set a member's metadata

     `metadata` is the caller's own JSON, free-form, kept as given.

    **Who:** signed in (a session or an API key).

    Args:
        org (str):
        account (str):
        body (MemberMetadataBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, MemberMetadataResponse200]
    """

    return sync_detailed(
        org=org,
        account=account,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    org: str,
    account: str,
    *,
    client: AuthenticatedClient,
    body: MemberMetadataBody,
) -> Response[Union[Error, MemberMetadataResponse200]]:
    """Set a member's metadata

     `metadata` is the caller's own JSON, free-form, kept as given.

    **Who:** signed in (a session or an API key).

    Args:
        org (str):
        account (str):
        body (MemberMetadataBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, MemberMetadataResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        account=account,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    org: str,
    account: str,
    *,
    client: AuthenticatedClient,
    body: MemberMetadataBody,
) -> Optional[Union[Error, MemberMetadataResponse200]]:
    """Set a member's metadata

     `metadata` is the caller's own JSON, free-form, kept as given.

    **Who:** signed in (a session or an API key).

    Args:
        org (str):
        account (str):
        body (MemberMetadataBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, MemberMetadataResponse200]
    """

    return (
        await asyncio_detailed(
            org=org,
            account=account,
            client=client,
            body=body,
        )
    ).parsed
