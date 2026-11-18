from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="AccountStatement")


@_attrs_define
class AccountStatement:
    """What an account proof states: the cluster and node, the account, the nonce and audience it binds, and when it was
    issued and expires (Unix seconds).

        Attributes:
            cluster (str):
            node (int):
            account (str):
            nonce (str):
            audience (str):
            issued_at (int):
            expires_at (int):
    """

    cluster: str
    node: int
    account: str
    nonce: str
    audience: str
    issued_at: int
    expires_at: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        cluster = self.cluster

        node = self.node

        account = self.account

        nonce = self.nonce

        audience = self.audience

        issued_at = self.issued_at

        expires_at = self.expires_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cluster": cluster,
                "node": node,
                "account": account,
                "nonce": nonce,
                "audience": audience,
                "issuedAt": issued_at,
                "expiresAt": expires_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        cluster = d.pop("cluster")

        node = d.pop("node")

        account = d.pop("account")

        nonce = d.pop("nonce")

        audience = d.pop("audience")

        issued_at = d.pop("issuedAt")

        expires_at = d.pop("expiresAt")

        account_statement = cls(
            cluster=cluster,
            node=node,
            account=account,
            nonce=nonce,
            audience=audience,
            issued_at=issued_at,
            expires_at=expires_at,
        )

        account_statement.additional_properties = d
        return account_statement

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
