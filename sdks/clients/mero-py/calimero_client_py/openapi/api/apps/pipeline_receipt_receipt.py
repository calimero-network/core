from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.signed_pipeline_receipt import SignedPipelineReceipt
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    app: str,
    run: str,
    *,
    version: Union[Unset, int] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["version"] = version

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/pipeline-runs/{run}/receipt".format(
            app=app,
            run=run,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, SignedPipelineReceipt]]:
    if response.status_code == 200:
        response_200 = SignedPipelineReceipt.from_dict(response.json())

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

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, SignedPipelineReceipt]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    run: str,
    *,
    client: AuthenticatedClient,
    version: Union[Unset, int] = UNSET,
) -> Response[Union[Error, SignedPipelineReceipt]]:
    """A pipeline run's receipt

     The owner and the organization's members. For an ended run: the receipt, the answering node and its
    signature over it, by the node's receipt key. `version=2` adds what each job's report signs to its
    proof (`job`) and each job node's attestation (`jobAttestations`); the default, `1`, leaves them
    out.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        run (str):
        version (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, SignedPipelineReceipt]]
    """

    kwargs = _get_kwargs(
        app=app,
        run=run,
        version=version,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    run: str,
    *,
    client: AuthenticatedClient,
    version: Union[Unset, int] = UNSET,
) -> Optional[Union[Error, SignedPipelineReceipt]]:
    """A pipeline run's receipt

     The owner and the organization's members. For an ended run: the receipt, the answering node and its
    signature over it, by the node's receipt key. `version=2` adds what each job's report signs to its
    proof (`job`) and each job node's attestation (`jobAttestations`); the default, `1`, leaves them
    out.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        run (str):
        version (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, SignedPipelineReceipt]
    """

    return sync_detailed(
        app=app,
        run=run,
        client=client,
        version=version,
    ).parsed


async def asyncio_detailed(
    app: str,
    run: str,
    *,
    client: AuthenticatedClient,
    version: Union[Unset, int] = UNSET,
) -> Response[Union[Error, SignedPipelineReceipt]]:
    """A pipeline run's receipt

     The owner and the organization's members. For an ended run: the receipt, the answering node and its
    signature over it, by the node's receipt key. `version=2` adds what each job's report signs to its
    proof (`job`) and each job node's attestation (`jobAttestations`); the default, `1`, leaves them
    out.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        run (str):
        version (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, SignedPipelineReceipt]]
    """

    kwargs = _get_kwargs(
        app=app,
        run=run,
        version=version,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    run: str,
    *,
    client: AuthenticatedClient,
    version: Union[Unset, int] = UNSET,
) -> Optional[Union[Error, SignedPipelineReceipt]]:
    """A pipeline run's receipt

     The owner and the organization's members. For an ended run: the receipt, the answering node and its
    signature over it, by the node's receipt key. `version=2` adds what each job's report signs to its
    proof (`job`) and each job node's attestation (`jobAttestations`); the default, `1`, leaves them
    out.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        run (str):
        version (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, SignedPipelineReceipt]
    """

    return (
        await asyncio_detailed(
            app=app,
            run=run,
            client=client,
            version=version,
        )
    ).parsed
