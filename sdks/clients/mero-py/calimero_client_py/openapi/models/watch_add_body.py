from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="WatchAddBody")


@_attrs_define
class WatchAddBody:
    """
    Attributes:
        stream (str):
        view (str):
        args (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
            it is).
    """

    stream: str
    view: str
    args: Union[Unset, Any] = UNSET

    def to_dict(self) -> dict[str, Any]:
        stream = self.stream

        view = self.view

        args = self.args

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "stream": stream,
                "view": view,
            }
        )
        if args is not UNSET:
            field_dict["args"] = args

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        stream = d.pop("stream")

        view = d.pop("view")

        args = d.pop("args", UNSET)

        watch_add_body = cls(
            stream=stream,
            view=view,
            args=args,
        )

        return watch_add_body
