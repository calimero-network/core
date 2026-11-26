from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.use_mode import UseMode
from typing import cast, Union

T = TypeVar("T", bound="LinkRow")


@_attrs_define
class LinkRow:
    """One link of an instance: the reference (`null` fields for one its release no longer names), the app linked, its
    release and whether it is in the reference's range (`null`: nothing linked).

        Attributes:
            name (str):
            package (Union[None, str]):
            range_ (Union[None, str]):
            mode (Union[None, UseMode]):
            app (Union[None, str]):
            version (Union[None, str]):
            in_range (Union[None, bool]):
    """

    name: str
    package: Union[None, str]
    range_: Union[None, str]
    mode: Union[None, UseMode]
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
        if isinstance(self.mode, UseMode):
            mode = self.mode.value
        else:
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

        def _parse_mode(data: object) -> Union[None, UseMode]:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                mode_type_0 = UseMode(data)

                return mode_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, UseMode], data)

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

        link_row = cls(
            name=name,
            package=package,
            range_=range_,
            mode=mode,
            app=app,
            version=version,
            in_range=in_range,
        )

        link_row.additional_properties = d
        return link_row

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
