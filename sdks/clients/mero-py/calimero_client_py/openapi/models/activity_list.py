from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.activity_view import ActivityView


T = TypeVar("T", bound="ActivityList")


@_attrs_define
class ActivityList:
    """`GET /auth/activity`: what the account's passkeys did, newest first.

    Attributes:
        activity (list['ActivityView']):
    """

    activity: list["ActivityView"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.activity_view import ActivityView

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
        from ..models.activity_view import ActivityView

        d = dict(src_dict)
        activity = []
        _activity = d.pop("activity")
        for activity_item_data in _activity:
            activity_item = ActivityView.from_dict(activity_item_data)

            activity.append(activity_item)

        activity_list = cls(
            activity=activity,
        )

        activity_list.additional_properties = d
        return activity_list

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
