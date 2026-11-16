from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast, Union
from typing import Union

T = TypeVar("T", bound="AppEntry")


@_attrs_define
class AppEntry:
    """An app as `GET /apps` lists it: `package` and `version` name the release its bundle came from (`null` for a raw
    upload), `deployment` the deployment it was created from (`null` for none; absent from an older node, as
    1.0.0-rc.1).

        Attributes:
            app_id (str):
            name (str):
            owner (str):
            org (Union[None, str]):
            bundle (str):
            package (Union[None, str]):
            version (Union[None, str]):
            deployment (Union[None, Unset, str]):
    """

    app_id: str
    name: str
    owner: str
    org: Union[None, str]
    bundle: str
    package: Union[None, str]
    version: Union[None, str]
    deployment: Union[None, Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        app_id = self.app_id

        name = self.name

        owner = self.owner

        org: Union[None, str]
        org = self.org

        bundle = self.bundle

        package: Union[None, str]
        package = self.package

        version: Union[None, str]
        version = self.version

        deployment: Union[None, Unset, str]
        if isinstance(self.deployment, Unset):
            deployment = UNSET
        else:
            deployment = self.deployment

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "appId": app_id,
                "name": name,
                "owner": owner,
                "org": org,
                "bundle": bundle,
                "package": package,
                "version": version,
            }
        )
        if deployment is not UNSET:
            field_dict["deployment"] = deployment

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        app_id = d.pop("appId")

        name = d.pop("name")

        owner = d.pop("owner")

        def _parse_org(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        org = _parse_org(d.pop("org"))

        bundle = d.pop("bundle")

        def _parse_package(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        package = _parse_package(d.pop("package"))

        def _parse_version(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        version = _parse_version(d.pop("version"))

        def _parse_deployment(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        deployment = _parse_deployment(d.pop("deployment", UNSET))

        app_entry = cls(
            app_id=app_id,
            name=name,
            owner=owner,
            org=org,
            bundle=bundle,
            package=package,
            version=version,
            deployment=deployment,
        )

        app_entry.additional_properties = d
        return app_entry

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
