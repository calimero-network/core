from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="ApikeyCreateBodyThenItem")


@_attrs_define
class ApikeyCreateBodyThenItem:
    """
    Attributes:
        app (str):
        method (str):
        args (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
            it is).
    """

    app: str
    method: str
    args: Union[Unset, Any] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        app = self.app

        method = self.method

        args = self.args

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "method": method,
            }
        )
        if args is not UNSET:
            field_dict["args"] = args

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        app = d.pop("app")

        method = d.pop("method")

        args = d.pop("args", UNSET)

        apikey_create_body_then_item = cls(
            app=app,
            method=method,
            args=args,
        )

        apikey_create_body_then_item.additional_properties = d
        return apikey_create_body_then_item

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
