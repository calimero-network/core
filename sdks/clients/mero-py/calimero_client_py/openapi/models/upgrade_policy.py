from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.upgrade_policy_auto_apply import UpgradePolicyAutoApply

T = TypeVar("T", bound="UpgradePolicy")


@_attrs_define
class UpgradePolicy:
    """How an organization's owner and admins let an app's updates apply (`POST /orgs/{org}/upgrade-policy`, `POST
    /apps/{app}/upgrade-policy`); every field is answered; a request may leave any out (its default). `secondApprover`
    (default off): a red update needs two distinct owners or admins. `autoApply` (default `never`; `noDestinations`: a
    green update of an app whose unit has no destinations before or after it, so no data leaves Calimero) and
    `holdHours` (24 to 168, default 24): such an update applies on its own once `holdHours` have passed since the
    release was logged (its install, or its deployment's offer), after a restore point.

        Attributes:
            auto_apply (UpgradePolicyAutoApply):
            hold_hours (int):
            second_approver (bool):
    """

    auto_apply: UpgradePolicyAutoApply
    hold_hours: int
    second_approver: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        auto_apply = self.auto_apply.value

        hold_hours = self.hold_hours

        second_approver = self.second_approver

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "autoApply": auto_apply,
                "holdHours": hold_hours,
                "secondApprover": second_approver,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        auto_apply = UpgradePolicyAutoApply(d.pop("autoApply"))

        hold_hours = d.pop("holdHours")

        second_approver = d.pop("secondApprover")

        upgrade_policy = cls(
            auto_apply=auto_apply,
            hold_hours=hold_hours,
            second_approver=second_approver,
        )

        upgrade_policy.additional_properties = d
        return upgrade_policy

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
