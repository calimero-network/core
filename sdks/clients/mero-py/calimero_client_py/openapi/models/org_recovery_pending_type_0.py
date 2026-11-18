from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="OrgRecoveryPendingType0")


@_attrs_define
class OrgRecoveryPendingType0:
    """
    Attributes:
        account (str):
        started_at (int):
        ready_at (int):
    """

    account: str
    started_at: int
    ready_at: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        account = self.account

        started_at = self.started_at

        ready_at = self.ready_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "account": account,
                "startedAt": started_at,
                "readyAt": ready_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        account = d.pop("account")

        started_at = d.pop("startedAt")

        ready_at = d.pop("readyAt")

        org_recovery_pending_type_0 = cls(
            account=account,
            started_at=started_at,
            ready_at=ready_at,
        )

        org_recovery_pending_type_0.additional_properties = d
        return org_recovery_pending_type_0

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
