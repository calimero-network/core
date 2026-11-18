from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RecoveryStartBody")


@_attrs_define
class RecoveryStartBody:
    """
    Attributes:
        nonce (str):
        signature (str):
    """

    nonce: str
    signature: str

    def to_dict(self) -> dict[str, Any]:
        nonce = self.nonce

        signature = self.signature

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "nonce": nonce,
                "signature": signature,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        nonce = d.pop("nonce")

        signature = d.pop("signature")

        recovery_start_body = cls(
            nonce=nonce,
            signature=signature,
        )

        return recovery_start_body
