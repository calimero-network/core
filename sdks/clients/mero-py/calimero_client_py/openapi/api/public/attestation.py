from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.attestation_doc import AttestationDoc
from ...models.attestation_purpose import AttestationPurpose
from ...models.error import Error
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    *,
    nonce: str,
    app: Union[Unset, str] = UNSET,
    app_hash: Union[Unset, str] = UNSET,
    purpose: Union[Unset, AttestationPurpose] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["nonce"] = nonce

    params["app"] = app

    params["appHash"] = app_hash

    json_purpose: Union[Unset, str] = UNSET
    if not isinstance(purpose, Unset):
        json_purpose = purpose.value

    params["purpose"] = json_purpose

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/attestation",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AttestationDoc, Error]]:
    if response.status_code == 200:
        response_200 = AttestationDoc.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AttestationDoc, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    nonce: str,
    app: Union[Unset, str] = UNSET,
    app_hash: Union[Unset, str] = UNSET,
    purpose: Union[Unset, AttestationPurpose] = UNSET,
) -> Response[Union[AttestationDoc, Error]]:
    """The node's attestation

     In the clear. A TDX quote binding the node's transport key (or, with `purpose=receipts`, its receipt
    key and the governance; with `purpose=kms`, what it verified of its KMS; with
    `purpose=orchestrator`, the cluster identity this replica holds) to `nonce`; `app`/`appHash` bind an
    app's bundle hash. A client checks it before it seals anything to the key.

    **Who:** anyone.

    Args:
        nonce (str):
        app (Union[Unset, str]):
        app_hash (Union[Unset, str]):
        purpose (Union[Unset, AttestationPurpose]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AttestationDoc, Error]]
    """

    kwargs = _get_kwargs(
        nonce=nonce,
        app=app,
        app_hash=app_hash,
        purpose=purpose,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    nonce: str,
    app: Union[Unset, str] = UNSET,
    app_hash: Union[Unset, str] = UNSET,
    purpose: Union[Unset, AttestationPurpose] = UNSET,
) -> Optional[Union[AttestationDoc, Error]]:
    """The node's attestation

     In the clear. A TDX quote binding the node's transport key (or, with `purpose=receipts`, its receipt
    key and the governance; with `purpose=kms`, what it verified of its KMS; with
    `purpose=orchestrator`, the cluster identity this replica holds) to `nonce`; `app`/`appHash` bind an
    app's bundle hash. A client checks it before it seals anything to the key.

    **Who:** anyone.

    Args:
        nonce (str):
        app (Union[Unset, str]):
        app_hash (Union[Unset, str]):
        purpose (Union[Unset, AttestationPurpose]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AttestationDoc, Error]
    """

    return sync_detailed(
        client=client,
        nonce=nonce,
        app=app,
        app_hash=app_hash,
        purpose=purpose,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    nonce: str,
    app: Union[Unset, str] = UNSET,
    app_hash: Union[Unset, str] = UNSET,
    purpose: Union[Unset, AttestationPurpose] = UNSET,
) -> Response[Union[AttestationDoc, Error]]:
    """The node's attestation

     In the clear. A TDX quote binding the node's transport key (or, with `purpose=receipts`, its receipt
    key and the governance; with `purpose=kms`, what it verified of its KMS; with
    `purpose=orchestrator`, the cluster identity this replica holds) to `nonce`; `app`/`appHash` bind an
    app's bundle hash. A client checks it before it seals anything to the key.

    **Who:** anyone.

    Args:
        nonce (str):
        app (Union[Unset, str]):
        app_hash (Union[Unset, str]):
        purpose (Union[Unset, AttestationPurpose]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AttestationDoc, Error]]
    """

    kwargs = _get_kwargs(
        nonce=nonce,
        app=app,
        app_hash=app_hash,
        purpose=purpose,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    nonce: str,
    app: Union[Unset, str] = UNSET,
    app_hash: Union[Unset, str] = UNSET,
    purpose: Union[Unset, AttestationPurpose] = UNSET,
) -> Optional[Union[AttestationDoc, Error]]:
    """The node's attestation

     In the clear. A TDX quote binding the node's transport key (or, with `purpose=receipts`, its receipt
    key and the governance; with `purpose=kms`, what it verified of its KMS; with
    `purpose=orchestrator`, the cluster identity this replica holds) to `nonce`; `app`/`appHash` bind an
    app's bundle hash. A client checks it before it seals anything to the key.

    **Who:** anyone.

    Args:
        nonce (str):
        app (Union[Unset, str]):
        app_hash (Union[Unset, str]):
        purpose (Union[Unset, AttestationPurpose]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AttestationDoc, Error]
    """

    return (
        await asyncio_detailed(
            client=client,
            nonce=nonce,
            app=app,
            app_hash=app_hash,
            purpose=purpose,
        )
    ).parsed
