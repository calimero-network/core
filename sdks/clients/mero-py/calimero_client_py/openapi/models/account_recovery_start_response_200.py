from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="AccountRecoveryStartResponse200")


@_attrs_define
class AccountRecoveryStartResponse200:
    """
    Attributes:
        account_id (str):
        credential_id (str):
        started_at (int):
        ready_at (int):
    """

    account_id: str
    credential_id: str
    started_at: int
    ready_at: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        account_id = self.account_id

        credential_id = self.credential_id

        started_at = self.started_at

        ready_at = self.ready_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "accountId": account_id,
                "credentialId": credential_id,
                "startedAt": started_at,
                "readyAt": ready_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        account_id = d.pop("accountId")

        credential_id = d.pop("credentialId")

        started_at = d.pop("startedAt")

        ready_at = d.pop("readyAt")

        account_recovery_start_response_200 = cls(
            account_id=account_id,
            credential_id=credential_id,
            started_at=started_at,
            ready_at=ready_at,
        )

        account_recovery_start_response_200.additional_properties = d
        return account_recovery_start_response_200

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
