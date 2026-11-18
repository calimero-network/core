from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.package import Package


T = TypeVar("T", bound="PackageListResponse200")


@_attrs_define
class PackageListResponse200:
    """
    Attributes:
        packages (list['Package']):
    """

    packages: list["Package"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package import Package

        packages = []
        for packages_item_data in self.packages:
            packages_item = packages_item_data.to_dict()
            packages.append(packages_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "packages": packages,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package import Package

        d = dict(src_dict)
        packages = []
        _packages = d.pop("packages")
        for packages_item_data in _packages:
            packages_item = Package.from_dict(packages_item_data)

            packages.append(packages_item)

        package_list_response_200 = cls(
            packages=packages,
        )

        package_list_response_200.additional_properties = d
        return package_list_response_200

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
