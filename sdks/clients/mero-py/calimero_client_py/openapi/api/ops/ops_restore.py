from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.ops_restore_body import OpsRestoreBody
from ...models.ops_restore_response_200 import OpsRestoreResponse200
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: OpsRestoreBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/ops/apps/{app}/restore".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OpsRestoreResponse200]]:
    if response.status_code == 200:
        response_200 = OpsRestoreResponse200.from_dict(response.json())

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

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

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
) -> Response[Union[Error, OpsRestoreResponse200]]:
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
    body: OpsRestoreBody,
) -> Response[Union[Error, OpsRestoreResponse200]]:
    """Restore an app from its sealed backup

     Signed over `<app> / __restore_app / { backup, sha256 }`. The receiving node holds the backup
    (`backup`, else the newest, in `{backup-dir}/{app}/`) and seeds the restore; answers once every
    replica holds the restored state: the rows restored, the backup (`format` 0, with `merod` and
    `release` `null`, for one written before manifests) and the app group's Raft status. `recovery`
    restores from a recovery-disk file instead (in the app's holder's directory under `--recover-from`,
    else `--recovery-dir`), opened with the data key its holder opened from it (hex); `backup` is then
    the file's name and `sha256` its body's.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsRestoreBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsRestoreResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
    body: OpsRestoreBody,
) -> Optional[Union[Error, OpsRestoreResponse200]]:
    """Restore an app from its sealed backup

     Signed over `<app> / __restore_app / { backup, sha256 }`. The receiving node holds the backup
    (`backup`, else the newest, in `{backup-dir}/{app}/`) and seeds the restore; answers once every
    replica holds the restored state: the rows restored, the backup (`format` 0, with `merod` and
    `release` `null`, for one written before manifests) and the app group's Raft status. `recovery`
    restores from a recovery-disk file instead (in the app's holder's directory under `--recover-from`,
    else `--recovery-dir`), opened with the data key its holder opened from it (hex); `backup` is then
    the file's name and `sha256` its body's.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsRestoreBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsRestoreResponse200]
    """

    return sync_detailed(
        app=app,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
    body: OpsRestoreBody,
) -> Response[Union[Error, OpsRestoreResponse200]]:
    """Restore an app from its sealed backup

     Signed over `<app> / __restore_app / { backup, sha256 }`. The receiving node holds the backup
    (`backup`, else the newest, in `{backup-dir}/{app}/`) and seeds the restore; answers once every
    replica holds the restored state: the rows restored, the backup (`format` 0, with `merod` and
    `release` `null`, for one written before manifests) and the app group's Raft status. `recovery`
    restores from a recovery-disk file instead (in the app's holder's directory under `--recover-from`,
    else `--recovery-dir`), opened with the data key its holder opened from it (hex); `backup` is then
    the file's name and `sha256` its body's.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsRestoreBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsRestoreResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
    body: OpsRestoreBody,
) -> Optional[Union[Error, OpsRestoreResponse200]]:
    """Restore an app from its sealed backup

     Signed over `<app> / __restore_app / { backup, sha256 }`. The receiving node holds the backup
    (`backup`, else the newest, in `{backup-dir}/{app}/`) and seeds the restore; answers once every
    replica holds the restored state: the rows restored, the backup (`format` 0, with `merod` and
    `release` `null`, for one written before manifests) and the app group's Raft status. `recovery`
    restores from a recovery-disk file instead (in the app's holder's directory under `--recover-from`,
    else `--recovery-dir`), opened with the data key its holder opened from it (hex); `backup` is then
    the file's name and `sha256` its body's.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsRestoreBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsRestoreResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
