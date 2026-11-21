from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.org_policy_set_body_policy_type_0_auto_apply import (
    OrgPolicySetBodyPolicyType0AutoApply,
)
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="OrgPolicySetBodyPolicyType0")


@_attrs_define
class OrgPolicySetBodyPolicyType0:
    """
    Attributes:
        auto_apply (Union[Unset, OrgPolicySetBodyPolicyType0AutoApply]):
        hold_hours (Union[Unset, int]):
        second_approver (Union[Unset, bool]):
    """

    auto_apply: Union[Unset, OrgPolicySetBodyPolicyType0AutoApply] = UNSET
    hold_hours: Union[Unset, int] = UNSET
    second_approver: Union[Unset, bool] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        auto_apply: Union[Unset, str] = UNSET
        if not isinstance(self.auto_apply, Unset):
            auto_apply = self.auto_apply.value

        hold_hours = self.hold_hours

        second_approver = self.second_approver

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if auto_apply is not UNSET:
            field_dict["autoApply"] = auto_apply
        if hold_hours is not UNSET:
            field_dict["holdHours"] = hold_hours
        if second_approver is not UNSET:
            field_dict["secondApprover"] = second_approver

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        _auto_apply = d.pop("autoApply", UNSET)
        auto_apply: Union[Unset, OrgPolicySetBodyPolicyType0AutoApply]
        if isinstance(_auto_apply, Unset):
            auto_apply = UNSET
        else:
            auto_apply = OrgPolicySetBodyPolicyType0AutoApply(_auto_apply)

        hold_hours = d.pop("holdHours", UNSET)

        second_approver = d.pop("secondApprover", UNSET)

        org_policy_set_body_policy_type_0 = cls(
            auto_apply=auto_apply,
            hold_hours=hold_hours,
            second_approver=second_approver,
        )

        org_policy_set_body_policy_type_0.additional_properties = d
        return org_policy_set_body_policy_type_0

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
