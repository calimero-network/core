from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="AppLinksResponse200LinksItem")


@_attrs_define
class AppLinksResponse200LinksItem:
    """
    Attributes:
        name (str):
        package (Union[None, str]):
        range_ (Union[None, str]):
        mode (Union[None, str]):
        app (Union[None, str]):
        version (Union[None, str]):
        in_range (Union[None, bool]):
    """

    name: str
    package: Union[None, str]
    range_: Union[None, str]
    mode: Union[None, str]
    app: Union[None, str]
    version: Union[None, str]
    in_range: Union[None, bool]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        name = self.name

        package: Union[None, str]
        package = self.package

        range_: Union[None, str]
        range_ = self.range_

        mode: Union[None, str]
        mode = self.mode

        app: Union[None, str]
        app = self.app

        version: Union[None, str]
        version = self.version

        in_range: Union[None, bool]
        in_range = self.in_range

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "name": name,
                "package": package,
                "range": range_,
                "mode": mode,
                "app": app,
                "version": version,
                "inRange": in_range,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        name = d.pop("name")

        def _parse_package(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        package = _parse_package(d.pop("package"))

        def _parse_range_(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        range_ = _parse_range_(d.pop("range"))

        def _parse_mode(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        mode = _parse_mode(d.pop("mode"))

        def _parse_app(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        app = _parse_app(d.pop("app"))

        def _parse_version(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        version = _parse_version(d.pop("version"))

        def _parse_in_range(data: object) -> Union[None, bool]:
            if data is None:
                return data
            return cast(Union[None, bool], data)

        in_range = _parse_in_range(d.pop("inRange"))

        app_links_response_200_links_item = cls(
            name=name,
            package=package,
            range_=range_,
            mode=mode,
            app=app,
            version=version,
            in_range=in_range,
        )

        app_links_response_200_links_item.additional_properties = d
        return app_links_response_200_links_item

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
