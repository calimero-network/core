from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.org_pending_upgrades_additional_property_class_class import (
    OrgPendingUpgradesAdditionalPropertyClassClass,
)
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.update_reason_type_6 import UpdateReasonType6
    from ..models.update_reason_type_4 import UpdateReasonType4
    from ..models.update_reason_type_3 import UpdateReasonType3
    from ..models.update_reason_type_0 import UpdateReasonType0
    from ..models.update_reason_type_5 import UpdateReasonType5
    from ..models.update_reason_type_2 import UpdateReasonType2
    from ..models.update_reason_type_1 import UpdateReasonType1


T = TypeVar("T", bound="OrgPendingUpgradesAdditionalPropertyClass")


@_attrs_define
class OrgPendingUpgradesAdditionalPropertyClass:
    """
    Attributes:
        class_ (OrgPendingUpgradesAdditionalPropertyClassClass):
        reasons (Union[Unset, list[Union['UpdateReasonType0', 'UpdateReasonType1', 'UpdateReasonType2',
            'UpdateReasonType3', 'UpdateReasonType4', 'UpdateReasonType5', 'UpdateReasonType6']]]):
    """

    class_: OrgPendingUpgradesAdditionalPropertyClassClass
    reasons: Union[
        Unset,
        list[
            Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
                "UpdateReasonType6",
            ]
        ],
    ] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_reason_type_6 import UpdateReasonType6
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2
        from ..models.update_reason_type_1 import UpdateReasonType1

        class_ = self.class_.value

        reasons: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.reasons, Unset):
            reasons = []
            for reasons_item_data in self.reasons:
                reasons_item: dict[str, Any]
                if isinstance(reasons_item_data, UpdateReasonType0):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType1):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType2):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType3):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType4):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType5):
                    reasons_item = reasons_item_data.to_dict()
                else:
                    reasons_item = reasons_item_data.to_dict()

                reasons.append(reasons_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "class": class_,
            }
        )
        if reasons is not UNSET:
            field_dict["reasons"] = reasons

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.update_reason_type_6 import UpdateReasonType6
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2
        from ..models.update_reason_type_1 import UpdateReasonType1

        d = dict(src_dict)
        class_ = OrgPendingUpgradesAdditionalPropertyClassClass(d.pop("class"))

        reasons = []
        _reasons = d.pop("reasons", UNSET)
        for reasons_item_data in _reasons or []:

            def _parse_reasons_item(
                data: object,
            ) -> Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
                "UpdateReasonType6",
            ]:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_0 = (
                        UpdateReasonType0.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_0
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_1 = (
                        UpdateReasonType1.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_1
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_2 = (
                        UpdateReasonType2.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_2
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_3 = (
                        UpdateReasonType3.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_3
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_4 = (
                        UpdateReasonType4.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_4
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_5 = (
                        UpdateReasonType5.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_5
                except:  # noqa: E722
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_update_reason_type_6 = UpdateReasonType6.from_dict(
                    data
                )

                return componentsschemas_update_reason_type_6

            reasons_item = _parse_reasons_item(reasons_item_data)

            reasons.append(reasons_item)

        org_pending_upgrades_additional_property_class = cls(
            class_=class_,
            reasons=reasons,
        )

        org_pending_upgrades_additional_property_class.additional_properties = d
        return org_pending_upgrades_additional_property_class

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
