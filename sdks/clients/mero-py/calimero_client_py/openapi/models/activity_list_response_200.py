from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.activity_list_response_200_activity_item import (
        ActivityListResponse200ActivityItem,
    )


T = TypeVar("T", bound="ActivityListResponse200")


@_attrs_define
class ActivityListResponse200:
    """
    Attributes:
        activity (list['ActivityListResponse200ActivityItem']):
    """

    activity: list["ActivityListResponse200ActivityItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.activity_list_response_200_activity_item import (
            ActivityListResponse200ActivityItem,
        )

        activity = []
        for activity_item_data in self.activity:
            activity_item = activity_item_data.to_dict()
            activity.append(activity_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "activity": activity,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.activity_list_response_200_activity_item import (
            ActivityListResponse200ActivityItem,
        )

        d = dict(src_dict)
        activity = []
        _activity = d.pop("activity")
        for activity_item_data in _activity:
            activity_item = ActivityListResponse200ActivityItem.from_dict(
                activity_item_data
            )

            activity.append(activity_item)

        activity_list_response_200 = cls(
            activity=activity,
        )

        activity_list_response_200.additional_properties = d
        return activity_list_response_200

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
