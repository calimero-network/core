from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_update_response_200_policy_source import (
    AppUpdateResponse200PolicySource,
)

T = TypeVar("T", bound="AppUpdateResponse200Policy")


@_attrs_define
class AppUpdateResponse200Policy:
    """
    Attributes:
        auto_sealed (bool):
        hold_hours (int):
        second_approver (bool):
        source (AppUpdateResponse200PolicySource):
    """

    auto_sealed: bool
    hold_hours: int
    second_approver: bool
    source: AppUpdateResponse200PolicySource
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        auto_sealed = self.auto_sealed

        hold_hours = self.hold_hours

        second_approver = self.second_approver

        source = self.source.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "autoSealed": auto_sealed,
                "holdHours": hold_hours,
                "secondApprover": second_approver,
                "source": source,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        auto_sealed = d.pop("autoSealed")

        hold_hours = d.pop("holdHours")

        second_approver = d.pop("secondApprover")

        source = AppUpdateResponse200PolicySource(d.pop("source"))

        app_update_response_200_policy = cls(
            auto_sealed=auto_sealed,
            hold_hours=hold_hours,
            second_approver=second_approver,
            source=source,
        )

        app_update_response_200_policy.additional_properties = d
        return app_update_response_200_policy

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
