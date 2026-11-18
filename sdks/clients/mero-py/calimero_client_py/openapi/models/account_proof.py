from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.account_statement import AccountStatement


T = TypeVar("T", bound="AccountProof")


@_attrs_define
class AccountProof:
    """A statement, signed with the answering node's receipt key (hex, Ed25519 over the statement's canonical JSON after
    its domain), that the account asked to bind `nonce` for `audience`, valid from `issuedAt` to `expiresAt` (Unix
    seconds).

        Attributes:
            statement (AccountStatement): What an account proof states: the cluster and node, the account, the nonce and
                audience it binds, and when it was issued and expires (Unix seconds).
            key (str):
            signature (str):
    """

    statement: "AccountStatement"
    key: str
    signature: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.account_statement import AccountStatement

        statement = self.statement.to_dict()

        key = self.key

        signature = self.signature

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "statement": statement,
                "key": key,
                "signature": signature,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.account_statement import AccountStatement

        d = dict(src_dict)
        statement = AccountStatement.from_dict(d.pop("statement"))

        key = d.pop("key")

        signature = d.pop("signature")

        account_proof = cls(
            statement=statement,
            key=key,
            signature=signature,
        )

        account_proof.additional_properties = d
        return account_proof

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
