from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.package_origin import PackageOrigin


T = TypeVar("T", bound="OriginsListResponse200")


@_attrs_define
class OriginsListResponse200:
    """
    Attributes:
        package (str):
        origins (list['PackageOrigin']):
    """

    package: str
    origins: list["PackageOrigin"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package_origin import PackageOrigin

        package = self.package

        origins = []
        for origins_item_data in self.origins:
            origins_item = origins_item_data.to_dict()
            origins.append(origins_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "origins": origins,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package_origin import PackageOrigin

        d = dict(src_dict)
        package = d.pop("package")

        origins = []
        _origins = d.pop("origins")
        for origins_item_data in _origins:
            origins_item = PackageOrigin.from_dict(origins_item_data)

            origins.append(origins_item)

        origins_list_response_200 = cls(
            package=package,
            origins=origins,
        )

        origins_list_response_200.additional_properties = d
        return origins_list_response_200

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
