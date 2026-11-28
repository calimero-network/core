from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="IntervalProof")


@_attrs_define
class IntervalProof:
    """With `form=interval`: the node's interval key (Ed25519, hex) that its quote binds, the quote's own nonce (hex) and
    when it was made (Unix seconds), and that key's signature (hex) over the request's nonce, app hash and transport
    key.

        Attributes:
            key (str):
            quote_nonce (str):
            issued_at (int):
            signature (str):
    """

    key: str
    quote_nonce: str
    issued_at: int
    signature: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        key = self.key

        quote_nonce = self.quote_nonce

        issued_at = self.issued_at

        signature = self.signature

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "key": key,
                "quoteNonce": quote_nonce,
                "issuedAt": issued_at,
                "signature": signature,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        key = d.pop("key")

        quote_nonce = d.pop("quoteNonce")

        issued_at = d.pop("issuedAt")

        signature = d.pop("signature")

        interval_proof = cls(
            key=key,
            quote_nonce=quote_nonce,
            issued_at=issued_at,
            signature=signature,
        )

        interval_proof.additional_properties = d
        return interval_proof

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
