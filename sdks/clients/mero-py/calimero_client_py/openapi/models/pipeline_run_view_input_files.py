from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.pipeline_run_view_input_files_additional_property import (
        PipelineRunViewInputFilesAdditionalProperty,
    )


T = TypeVar("T", bound="PipelineRunViewInputFiles")


@_attrs_define
class PipelineRunViewInputFiles:
    """ """

    additional_properties: dict[str, "PipelineRunViewInputFilesAdditionalProperty"] = (
        _attrs_field(init=False, factory=dict)
    )

    def to_dict(self) -> dict[str, Any]:
        from ..models.pipeline_run_view_input_files_additional_property import (
            PipelineRunViewInputFilesAdditionalProperty,
        )

        field_dict: dict[str, Any] = {}
        for prop_name, prop in self.additional_properties.items():
            field_dict[prop_name] = prop.to_dict()

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pipeline_run_view_input_files_additional_property import (
            PipelineRunViewInputFilesAdditionalProperty,
        )

        d = dict(src_dict)
        pipeline_run_view_input_files = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():
            additional_property = PipelineRunViewInputFilesAdditionalProperty.from_dict(
                prop_dict
            )

            additional_properties[prop_name] = additional_property

        pipeline_run_view_input_files.additional_properties = additional_properties
        return pipeline_run_view_input_files

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> "PipelineRunViewInputFilesAdditionalProperty":
        return self.additional_properties[key]

    def __setitem__(
        self, key: str, value: "PipelineRunViewInputFilesAdditionalProperty"
    ) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
