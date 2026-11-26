from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.package_installed_services import PackageInstalledServices


T = TypeVar("T", bound="PackageInstalled")


@_attrs_define
class PackageInstalled:
    """A release installed: its package, version, pinned signer and services (name to bundle hash).

    Attributes:
        package (str):
        version (str):
        signer_id (str):
        services (PackageInstalledServices):
    """

    package: str
    version: str
    signer_id: str
    services: "PackageInstalledServices"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package_installed_services import PackageInstalledServices

        package = self.package

        version = self.version

        signer_id = self.signer_id

        services = self.services.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
                "signerId": signer_id,
                "services": services,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package_installed_services import PackageInstalledServices

        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        signer_id = d.pop("signerId")

        services = PackageInstalledServices.from_dict(d.pop("services"))

        package_installed = cls(
            package=package,
            version=version,
            signer_id=signer_id,
            services=services,
        )

        package_installed.additional_properties = d
        return package_installed

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
