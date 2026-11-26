from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.update_view_policy_auto_apply import UpdateViewPolicyAutoApply
from ..models.update_view_policy_source import UpdateViewPolicySource

T = TypeVar("T", bound="UpdateViewPolicy")


@_attrs_define
class UpdateViewPolicy:
    """
    Attributes:
        auto_apply (UpdateViewPolicyAutoApply):
        hold_hours (int):
        second_approver (bool):
        source (UpdateViewPolicySource):
    """

    auto_apply: UpdateViewPolicyAutoApply
    hold_hours: int
    second_approver: bool
    source: UpdateViewPolicySource
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        auto_apply = self.auto_apply.value

        hold_hours = self.hold_hours

        second_approver = self.second_approver

        source = self.source.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "autoApply": auto_apply,
                "holdHours": hold_hours,
                "secondApprover": second_approver,
                "source": source,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        auto_apply = UpdateViewPolicyAutoApply(d.pop("autoApply"))

        hold_hours = d.pop("holdHours")

        second_approver = d.pop("secondApprover")

        source = UpdateViewPolicySource(d.pop("source"))

        update_view_policy = cls(
            auto_apply=auto_apply,
            hold_hours=hold_hours,
            second_approver=second_approver,
            source=source,
        )

        update_view_policy.additional_properties = d
        return update_view_policy

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
