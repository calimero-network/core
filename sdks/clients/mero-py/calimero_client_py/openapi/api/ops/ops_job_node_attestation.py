from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.ops_job_node_attestation_response_200_type_0 import (
    OpsJobNodeAttestationResponse200Type0,
)
from ...models.ops_job_node_attestation_response_200_type_1 import (
    OpsJobNodeAttestationResponse200Type1,
)
from typing import cast
from typing import cast, Union


def _get_kwargs(
    id: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/ops/job-nodes/{id}/attestation".format(
            id=id,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[
    Union[
        Error,
        Union[
            "OpsJobNodeAttestationResponse200Type0",
            "OpsJobNodeAttestationResponse200Type1",
        ],
    ]
]:
    if response.status_code == 200:

        def _parse_response_200(
            data: object,
        ) -> Union[
            "OpsJobNodeAttestationResponse200Type0",
            "OpsJobNodeAttestationResponse200Type1",
        ]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                response_200_type_0 = OpsJobNodeAttestationResponse200Type0.from_dict(
                    data
                )

                return response_200_type_0
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            response_200_type_1 = OpsJobNodeAttestationResponse200Type1.from_dict(data)

            return response_200_type_1

        response_200 = _parse_response_200(response.json())

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

    if response.status_code == 503:
        response_503 = Error.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[
    Union[
        Error,
        Union[
            "OpsJobNodeAttestationResponse200Type0",
            "OpsJobNodeAttestationResponse200Type1",
        ],
    ]
]:
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
) -> Response[
    Union[
        Error,
        Union[
            "OpsJobNodeAttestationResponse200Type0",
            "OpsJobNodeAttestationResponse200Type1",
        ],
    ]
]:
    """Whether the cluster accepts a job node

     Runs the check the leader makes before it sends a job node work: a quote that passes the release
    policy, this cluster and node, this cluster's identity, a release not retired (not the per-app
    release approval). `admitted` carries the attested `release`; `refused` (final), with the reason: a
    400 or 404 from the node, a 2xx that is oversized or does not decode, a failed verification, or a
    400 or 404 from the collateral source on the PCK certificate or TCB info request; `unreachable`
    (transient), with the reason: anything else, including no answer, any other status from the node and
    any other collateral failure. `503` when this node's directory is stale. A verified answer is reused
    for up to a minute.

    **Who:** the ops token.

    Args:
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, Union['OpsJobNodeAttestationResponse200Type0', 'OpsJobNodeAttestationResponse200Type1']]]
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
) -> Optional[
    Union[
        Error,
        Union[
            "OpsJobNodeAttestationResponse200Type0",
            "OpsJobNodeAttestationResponse200Type1",
        ],
    ]
]:
    """Whether the cluster accepts a job node

     Runs the check the leader makes before it sends a job node work: a quote that passes the release
    policy, this cluster and node, this cluster's identity, a release not retired (not the per-app
    release approval). `admitted` carries the attested `release`; `refused` (final), with the reason: a
    400 or 404 from the node, a 2xx that is oversized or does not decode, a failed verification, or a
    400 or 404 from the collateral source on the PCK certificate or TCB info request; `unreachable`
    (transient), with the reason: anything else, including no answer, any other status from the node and
    any other collateral failure. `503` when this node's directory is stale. A verified answer is reused
    for up to a minute.

    **Who:** the ops token.

    Args:
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, Union['OpsJobNodeAttestationResponse200Type0', 'OpsJobNodeAttestationResponse200Type1']]
    """

    return sync_detailed(
        id=id,
        client=client,
    ).parsed


async def asyncio_detailed(
    id: str,
    *,
    client: AuthenticatedClient,
) -> Response[
    Union[
        Error,
        Union[
            "OpsJobNodeAttestationResponse200Type0",
            "OpsJobNodeAttestationResponse200Type1",
        ],
    ]
]:
    """Whether the cluster accepts a job node

     Runs the check the leader makes before it sends a job node work: a quote that passes the release
    policy, this cluster and node, this cluster's identity, a release not retired (not the per-app
    release approval). `admitted` carries the attested `release`; `refused` (final), with the reason: a
    400 or 404 from the node, a 2xx that is oversized or does not decode, a failed verification, or a
    400 or 404 from the collateral source on the PCK certificate or TCB info request; `unreachable`
    (transient), with the reason: anything else, including no answer, any other status from the node and
    any other collateral failure. `503` when this node's directory is stale. A verified answer is reused
    for up to a minute.

    **Who:** the ops token.

    Args:
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, Union['OpsJobNodeAttestationResponse200Type0', 'OpsJobNodeAttestationResponse200Type1']]]
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
) -> Optional[
    Union[
        Error,
        Union[
            "OpsJobNodeAttestationResponse200Type0",
            "OpsJobNodeAttestationResponse200Type1",
        ],
    ]
]:
    """Whether the cluster accepts a job node

     Runs the check the leader makes before it sends a job node work: a quote that passes the release
    policy, this cluster and node, this cluster's identity, a release not retired (not the per-app
    release approval). `admitted` carries the attested `release`; `refused` (final), with the reason: a
    400 or 404 from the node, a 2xx that is oversized or does not decode, a failed verification, or a
    400 or 404 from the collateral source on the PCK certificate or TCB info request; `unreachable`
    (transient), with the reason: anything else, including no answer, any other status from the node and
    any other collateral failure. `503` when this node's directory is stale. A verified answer is reused
    for up to a minute.

    **Who:** the ops token.

    Args:
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, Union['OpsJobNodeAttestationResponse200Type0', 'OpsJobNodeAttestationResponse200Type1']]
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
        )
    ).parsed
