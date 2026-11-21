from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.org_pending_upgrades_additional_property_targets import (
        OrgPendingUpgradesAdditionalPropertyTargets,
    )
    from ..models.org_pending_upgrades_additional_property_class import (
        OrgPendingUpgradesAdditionalPropertyClass,
    )


T = TypeVar("T", bound="OrgPendingUpgradesAdditionalProperty")


@_attrs_define
class OrgPendingUpgradesAdditionalProperty:
    """
    Attributes:
        package (str):
        version (str):
        by (str):
        at (int):
        class_ (OrgPendingUpgradesAdditionalPropertyClass):
        targets (Union[Unset, OrgPendingUpgradesAdditionalPropertyTargets]):
    """

    package: str
    version: str
    by: str
    at: int
    class_: "OrgPendingUpgradesAdditionalPropertyClass"
    targets: Union[Unset, "OrgPendingUpgradesAdditionalPropertyTargets"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.org_pending_upgrades_additional_property_targets import (
            OrgPendingUpgradesAdditionalPropertyTargets,
        )
        from ..models.org_pending_upgrades_additional_property_class import (
            OrgPendingUpgradesAdditionalPropertyClass,
        )

        package = self.package

        version = self.version

        by = self.by

        at = self.at

        class_ = self.class_.to_dict()

        targets: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.targets, Unset):
            targets = self.targets.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
                "by": by,
                "at": at,
                "class": class_,
            }
        )
        if targets is not UNSET:
            field_dict["targets"] = targets

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.org_pending_upgrades_additional_property_targets import (
            OrgPendingUpgradesAdditionalPropertyTargets,
        )
        from ..models.org_pending_upgrades_additional_property_class import (
            OrgPendingUpgradesAdditionalPropertyClass,
        )

        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        by = d.pop("by")

        at = d.pop("at")

        class_ = OrgPendingUpgradesAdditionalPropertyClass.from_dict(d.pop("class"))

        _targets = d.pop("targets", UNSET)
        targets: Union[Unset, OrgPendingUpgradesAdditionalPropertyTargets]
        if isinstance(_targets, Unset):
            targets = UNSET
        else:
            targets = OrgPendingUpgradesAdditionalPropertyTargets.from_dict(_targets)

        org_pending_upgrades_additional_property = cls(
            package=package,
            version=version,
            by=by,
            at=at,
            class_=class_,
            targets=targets,
        )

        org_pending_upgrades_additional_property.additional_properties = d
        return org_pending_upgrades_additional_property

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
