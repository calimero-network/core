from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.account_proof import AccountProof
from ...models.account_proof_prove_body import AccountProofProveBody
from ...models.error import Error
from typing import cast


def _get_kwargs(
    *,
    body: AccountProofProveBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/auth/account-proof",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AccountProof, Error]]:
    if response.status_code == 200:
        response_200 = AccountProof.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AccountProof, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: AccountProofProveBody,
) -> Response[Union[AccountProof, Error]]:
    """Prove the account to another party

     A statement signed with this node's receipt key that the caller's passkey session asked to bind
    `nonce` for `audience`.

    **Who:** a passkey session (an API key is refused).

    Args:
        body (AccountProofProveBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AccountProof, Error]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    body: AccountProofProveBody,
) -> Optional[Union[AccountProof, Error]]:
    """Prove the account to another party

     A statement signed with this node's receipt key that the caller's passkey session asked to bind
    `nonce` for `audience`.

    **Who:** a passkey session (an API key is refused).

    Args:
        body (AccountProofProveBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AccountProof, Error]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: AccountProofProveBody,
) -> Response[Union[AccountProof, Error]]:
    """Prove the account to another party

     A statement signed with this node's receipt key that the caller's passkey session asked to bind
    `nonce` for `audience`.

    **Who:** a passkey session (an API key is refused).

    Args:
        body (AccountProofProveBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AccountProof, Error]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: AccountProofProveBody,
) -> Optional[Union[AccountProof, Error]]:
    """Prove the account to another party

     A statement signed with this node's receipt key that the caller's passkey session asked to bind
    `nonce` for `audience`.

    **Who:** a passkey session (an API key is refused).

    Args:
        body (AccountProofProveBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AccountProof, Error]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
