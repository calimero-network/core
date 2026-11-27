from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.package_origin import PackageOrigin


T = TypeVar("T", bound="AppOriginList")


@_attrs_define
class AppOriginList:
    """`GET /apps/{app}/origins`: the origins registered for an instance.

    Attributes:
        app (str):
        origins (list['PackageOrigin']):
    """

    app: str
    origins: list["PackageOrigin"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package_origin import PackageOrigin

        app = self.app

        origins = []
        for origins_item_data in self.origins:
            origins_item = origins_item_data.to_dict()
            origins.append(origins_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "origins": origins,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package_origin import PackageOrigin

        d = dict(src_dict)
        app = d.pop("app")

        origins = []
        _origins = d.pop("origins")
        for origins_item_data in _origins:
            origins_item = PackageOrigin.from_dict(origins_item_data)

            origins.append(origins_item)

        app_origin_list = cls(
            app=app,
            origins=origins,
        )

        app_origin_list.additional_properties = d
        return app_origin_list

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
