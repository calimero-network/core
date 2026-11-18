from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="OperatorSignatureRequired")


@_attrs_define
class OperatorSignatureRequired:
    """`details` of a `428 OPERATOR_SIGNATURE_REQUIRED`: the statement to sign (`args` is the JSON text of its arguments,
    as signed) and the exact bytes to sign (`message`, standard base64).

        Attributes:
            subject (str):
            method (str):
            args (str):
            nonce (str):
            cluster (str):
            message (str):
    """

    subject: str
    method: str
    args: str
    nonce: str
    cluster: str
    message: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        subject = self.subject

        method = self.method

        args = self.args

        nonce = self.nonce

        cluster = self.cluster

        message = self.message

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "subject": subject,
                "method": method,
                "args": args,
                "nonce": nonce,
                "cluster": cluster,
                "message": message,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        subject = d.pop("subject")

        method = d.pop("method")

        args = d.pop("args")

        nonce = d.pop("nonce")

        cluster = d.pop("cluster")

        message = d.pop("message")

        operator_signature_required = cls(
            subject=subject,
            method=method,
            args=args,
            nonce=nonce,
            cluster=cluster,
            message=message,
        )

        operator_signature_required.additional_properties = d
        return operator_signature_required

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
