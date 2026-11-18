from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="PackageRef")


@_attrs_define
class PackageRef:
    """One service of one package release: `service` is absent for a single-service bundle, `compute` (absent: false) marks
    the service's compute component.

        Attributes:
            package (str):
            version (str):
            service (Union[Unset, str]):
            compute (Union[Unset, bool]):
    """

    package: str
    version: str
    service: Union[Unset, str] = UNSET
    compute: Union[Unset, bool] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        package = self.package

        version = self.version

        service = self.service

        compute = self.compute

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
            }
        )
        if service is not UNSET:
            field_dict["service"] = service
        if compute is not UNSET:
            field_dict["compute"] = compute

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        service = d.pop("service", UNSET)

        compute = d.pop("compute", UNSET)

        package_ref = cls(
            package=package,
            version=version,
            service=service,
            compute=compute,
        )

        package_ref.additional_properties = d
        return package_ref

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
