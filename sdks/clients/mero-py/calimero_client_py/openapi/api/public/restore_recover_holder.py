from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.restore_recover_holder_body import RestoreRecoverHolderBody
from ...models.restore_recover_holder_response_200 import (
    RestoreRecoverHolderResponse200,
)
from typing import cast


def _get_kwargs(
    *,
    body: RestoreRecoverHolderBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/recover/holder",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, RestoreRecoverHolderResponse200]]:
    if response.status_code == 200:
        response_200 = RestoreRecoverHolderResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 409:
        response_409 = Error.from_dict(response.json())

        return response_409

    if response.status_code == 428:
        response_428 = Error.from_dict(response.json())

        return response_428

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, RestoreRecoverHolderResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: RestoreRecoverHolderBody,
) -> Response[Union[Error, RestoreRecoverHolderResponse200]]:
    """Bring an organization's or account's apps back from its recovery files

     Sent by the holder's own device, over the sealed channel: its newest `holder` file (`slice`), each
    app's file and the code files, in the receiving node's `--recovery-dir` (or `--recover-from`) under
    `org-<id>/` or `account-<id>/`, each with the data key the holder opened from it with its backup key
    (hex). The cluster admits it with the operator's signature over `cluster / __recover_holder / {
    holder, slice, apps: [{ app, file, sha256 }] }` (`428` first, naming it). Where owners upgrade apps,
    and only for an organization, account, apps and passkeys the cluster does not hold; the receiving
    node seeds each app, which answers `409 APP_RESTORING` until its replicas hold it. People sign in
    again with the passkeys they had.

    **Who:** anyone. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it
    again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

    Args:
        body (RestoreRecoverHolderBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, RestoreRecoverHolderResponse200]]
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
    client: Union[AuthenticatedClient, Client],
    body: RestoreRecoverHolderBody,
) -> Optional[Union[Error, RestoreRecoverHolderResponse200]]:
    """Bring an organization's or account's apps back from its recovery files

     Sent by the holder's own device, over the sealed channel: its newest `holder` file (`slice`), each
    app's file and the code files, in the receiving node's `--recovery-dir` (or `--recover-from`) under
    `org-<id>/` or `account-<id>/`, each with the data key the holder opened from it with its backup key
    (hex). The cluster admits it with the operator's signature over `cluster / __recover_holder / {
    holder, slice, apps: [{ app, file, sha256 }] }` (`428` first, naming it). Where owners upgrade apps,
    and only for an organization, account, apps and passkeys the cluster does not hold; the receiving
    node seeds each app, which answers `409 APP_RESTORING` until its replicas hold it. People sign in
    again with the passkeys they had.

    **Who:** anyone. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it
    again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

    Args:
        body (RestoreRecoverHolderBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, RestoreRecoverHolderResponse200]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: RestoreRecoverHolderBody,
) -> Response[Union[Error, RestoreRecoverHolderResponse200]]:
    """Bring an organization's or account's apps back from its recovery files

     Sent by the holder's own device, over the sealed channel: its newest `holder` file (`slice`), each
    app's file and the code files, in the receiving node's `--recovery-dir` (or `--recover-from`) under
    `org-<id>/` or `account-<id>/`, each with the data key the holder opened from it with its backup key
    (hex). The cluster admits it with the operator's signature over `cluster / __recover_holder / {
    holder, slice, apps: [{ app, file, sha256 }] }` (`428` first, naming it). Where owners upgrade apps,
    and only for an organization, account, apps and passkeys the cluster does not hold; the receiving
    node seeds each app, which answers `409 APP_RESTORING` until its replicas hold it. People sign in
    again with the passkeys they had.

    **Who:** anyone. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it
    again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

    Args:
        body (RestoreRecoverHolderBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, RestoreRecoverHolderResponse200]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: RestoreRecoverHolderBody,
) -> Optional[Union[Error, RestoreRecoverHolderResponse200]]:
    """Bring an organization's or account's apps back from its recovery files

     Sent by the holder's own device, over the sealed channel: its newest `holder` file (`slice`), each
    app's file and the code files, in the receiving node's `--recovery-dir` (or `--recover-from`) under
    `org-<id>/` or `account-<id>/`, each with the data key the holder opened from it with its backup key
    (hex). The cluster admits it with the operator's signature over `cluster / __recover_holder / {
    holder, slice, apps: [{ app, file, sha256 }] }` (`428` first, naming it). Where owners upgrade apps,
    and only for an organization, account, apps and passkeys the cluster does not hold; the receiving
    node seeds each app, which answers `409 APP_RESTORING` until its replicas hold it. People sign in
    again with the passkeys they had.

    **Who:** anyone. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it
    again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

    Args:
        body (RestoreRecoverHolderBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, RestoreRecoverHolderResponse200]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
