from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="AccountProofProveBody")


@_attrs_define
class AccountProofProveBody:
    """
    Attributes:
        nonce (str):
        audience (str):
    """

    nonce: str
    audience: str

    def to_dict(self) -> dict[str, Any]:
        nonce = self.nonce

        audience = self.audience

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "nonce": nonce,
                "audience": audience,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        nonce = d.pop("nonce")

        audience = d.pop("audience")

        account_proof_prove_body = cls(
            nonce=nonce,
            audience=audience,
        )

        return account_proof_prove_body
