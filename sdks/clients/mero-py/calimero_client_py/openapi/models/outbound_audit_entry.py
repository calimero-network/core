from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.outbound_audit_entry_failure import OutboundAuditEntryFailure
from ..types import UNSET, Unset
from typing import cast
from typing import Union

T = TypeVar("T", bound="OutboundAuditEntry")


@_attrs_define
class OutboundAuditEntry:
    """One outbound request of an instance: `atMs` (Unix milliseconds, the node's clock), `method`, `host` and `path` (no
    query string, at most 128 bytes, `pathTruncated` when cut; both empty for a URL that did not parse), the host's
    `status` or, absent that, the `failure` that stopped it (`malformed`, `refused` by a gate, `notApproved`,
    `awaitingApproval`, `secret` not filled in, `failed` in transit), the names of the `secrets` it asked for, and the
    request's and response's body sizes. Never a secret value, a body, a query string or a header.

        Attributes:
            at_ms (int):
            method (str):
            host (str):
            path (str):
            path_truncated (bool):
            secrets (list[str]):
            request_bytes (int):
            response_bytes (int):
            status (Union[Unset, int]):
            failure (Union[Unset, OutboundAuditEntryFailure]):
    """

    at_ms: int
    method: str
    host: str
    path: str
    path_truncated: bool
    secrets: list[str]
    request_bytes: int
    response_bytes: int
    status: Union[Unset, int] = UNSET
    failure: Union[Unset, OutboundAuditEntryFailure] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        at_ms = self.at_ms

        method = self.method

        host = self.host

        path = self.path

        path_truncated = self.path_truncated

        secrets = self.secrets

        request_bytes = self.request_bytes

        response_bytes = self.response_bytes

        status = self.status

        failure: Union[Unset, str] = UNSET
        if not isinstance(self.failure, Unset):
            failure = self.failure.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "atMs": at_ms,
                "method": method,
                "host": host,
                "path": path,
                "pathTruncated": path_truncated,
                "secrets": secrets,
                "requestBytes": request_bytes,
                "responseBytes": response_bytes,
            }
        )
        if status is not UNSET:
            field_dict["status"] = status
        if failure is not UNSET:
            field_dict["failure"] = failure

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        at_ms = d.pop("atMs")

        method = d.pop("method")

        host = d.pop("host")

        path = d.pop("path")

        path_truncated = d.pop("pathTruncated")

        secrets = cast(list[str], d.pop("secrets"))

        request_bytes = d.pop("requestBytes")

        response_bytes = d.pop("responseBytes")

        status = d.pop("status", UNSET)

        _failure = d.pop("failure", UNSET)
        failure: Union[Unset, OutboundAuditEntryFailure]
        if isinstance(_failure, Unset):
            failure = UNSET
        else:
            failure = OutboundAuditEntryFailure(_failure)

        outbound_audit_entry = cls(
            at_ms=at_ms,
            method=method,
            host=host,
            path=path,
            path_truncated=path_truncated,
            secrets=secrets,
            request_bytes=request_bytes,
            response_bytes=response_bytes,
            status=status,
            failure=failure,
        )

        outbound_audit_entry.additional_properties = d
        return outbound_audit_entry

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
