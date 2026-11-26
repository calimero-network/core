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
        interval_ms (Union[Unset, int]):
    """

    stream: str
    view: str
    args: Union[Unset, Any] = UNSET
    interval_ms: Union[Unset, int] = UNSET

    def to_dict(self) -> dict[str, Any]:
        stream = self.stream

        view = self.view

        args = self.args

        interval_ms = self.interval_ms

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "stream": stream,
                "view": view,
            }
        )
        if args is not UNSET:
            field_dict["args"] = args
        if interval_ms is not UNSET:
            field_dict["intervalMs"] = interval_ms

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        stream = d.pop("stream")

        view = d.pop("view")

        args = d.pop("args", UNSET)

        interval_ms = d.pop("intervalMs", UNSET)

        watch_add_body = cls(
            stream=stream,
            view=view,
            args=args,
            interval_ms=interval_ms,
        )

        return watch_add_body
