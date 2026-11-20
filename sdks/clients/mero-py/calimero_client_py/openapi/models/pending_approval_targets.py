from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.pending_approval_targets_additional_property import (
        PendingApprovalTargetsAdditionalProperty,
    )


T = TypeVar("T", bound="PendingApprovalTargets")


@_attrs_define
class PendingApprovalTargets:
    """ """

    additional_properties: dict[str, "PendingApprovalTargetsAdditionalProperty"] = (
        _attrs_field(init=False, factory=dict)
    )

    def to_dict(self) -> dict[str, Any]:
        from ..models.pending_approval_targets_additional_property import (
            PendingApprovalTargetsAdditionalProperty,
        )

        field_dict: dict[str, Any] = {}
        for prop_name, prop in self.additional_properties.items():
            field_dict[prop_name] = prop.to_dict()

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pending_approval_targets_additional_property import (
            PendingApprovalTargetsAdditionalProperty,
        )

        d = dict(src_dict)
        pending_approval_targets = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():
            additional_property = PendingApprovalTargetsAdditionalProperty.from_dict(
                prop_dict
            )

            additional_properties[prop_name] = additional_property

        pending_approval_targets.additional_properties = additional_properties
        return pending_approval_targets

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> "PendingApprovalTargetsAdditionalProperty":
        return self.additional_properties[key]

    def __setitem__(
        self, key: str, value: "PendingApprovalTargetsAdditionalProperty"
    ) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
