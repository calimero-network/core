from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.app_restored import AppRestored


T = TypeVar("T", bound="HolderRecovered")


@_attrs_define
class HolderRecovered:
    """What `POST /recover/holder` answers: each app restored from the holder's files.

    Attributes:
        apps (list['AppRestored']):
    """

    apps: list["AppRestored"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_restored import AppRestored

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "apps": apps,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_restored import AppRestored

        d = dict(src_dict)
        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = AppRestored.from_dict(apps_item_data)

            apps.append(apps_item)

        holder_recovered = cls(
            apps=apps,
        )

        holder_recovered.additional_properties = d
        return holder_recovered

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
