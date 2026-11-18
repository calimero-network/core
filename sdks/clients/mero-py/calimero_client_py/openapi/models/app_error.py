from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="AppError")


@_attrs_define
class AppError:
    """`details` of a `422 APP_ERROR`: the app's error code, its fields (the app's own value, free-form: the object its ABI
    declares for the error, or the app's message as a string when that is not JSON), and the log index of the call
    (absent when the node refused it before it ran: `UnknownMethod`, `WrongKind`, `Forbidden`, `ConfirmNotAView`,
    `NotAView`, `ConfirmRequired`).

        Attributes:
            code (str):
            fields (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            index (Union[Unset, int]):
    """

    code: str
    fields: Any
    index: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        code = self.code

        fields = self.fields

        index = self.index

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "code": code,
                "fields": fields,
            }
        )
        if index is not UNSET:
            field_dict["index"] = index

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        code = d.pop("code")

        fields = d.pop("fields")

        index = d.pop("index", UNSET)

        app_error = cls(
            code=code,
            fields=fields,
            index=index,
        )

        app_error.additional_properties = d
        return app_error

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
