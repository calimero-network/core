from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.package_versions_item import PackageVersionsItem


T = TypeVar("T", bound="Package")


@_attrs_define
class Package:
    """A package and the releases of it the caller may see, oldest first; `latest` is the newest of those.

    Attributes:
        package (str):
        signer_id (str):
        owner (str):
        latest (str):
        versions (list['PackageVersionsItem']):
    """

    package: str
    signer_id: str
    owner: str
    latest: str
    versions: list["PackageVersionsItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package_versions_item import PackageVersionsItem

        package = self.package

        signer_id = self.signer_id

        owner = self.owner

        latest = self.latest

        versions = []
        for versions_item_data in self.versions:
            versions_item = versions_item_data.to_dict()
            versions.append(versions_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "signerId": signer_id,
                "owner": owner,
                "latest": latest,
                "versions": versions,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package_versions_item import PackageVersionsItem

        d = dict(src_dict)
        package = d.pop("package")

        signer_id = d.pop("signerId")

        owner = d.pop("owner")

        latest = d.pop("latest")

        versions = []
        _versions = d.pop("versions")
        for versions_item_data in _versions:
            versions_item = PackageVersionsItem.from_dict(versions_item_data)

            versions.append(versions_item)

        package = cls(
            package=package,
            signer_id=signer_id,
            owner=owner,
            latest=latest,
            versions=versions,
        )

        package.additional_properties = d
        return package

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
