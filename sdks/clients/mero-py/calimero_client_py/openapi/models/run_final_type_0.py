from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.run_final_type_0_outcome_type_2 import RunFinalType0OutcomeType2
    from ..models.run_final_type_0_usage import RunFinalType0Usage
    from ..models.run_final_type_0_outcome_type_0 import RunFinalType0OutcomeType0
    from ..models.run_final_type_0_outcome_type_1 import RunFinalType0OutcomeType1
    from ..models.run_file import RunFile


T = TypeVar("T", bound="RunFinalType0")


@_attrs_define
class RunFinalType0:
    """
    Attributes:
        attempt (int):
        outcome (Union['RunFinalType0OutcomeType0', 'RunFinalType0OutcomeType1', 'RunFinalType0OutcomeType2']):
        outputs (list['RunFile']):
        usage (RunFinalType0Usage):
        ts_ms (int):
        message (Union[None, str]):
    """

    attempt: int
    outcome: Union[
        "RunFinalType0OutcomeType0",
        "RunFinalType0OutcomeType1",
        "RunFinalType0OutcomeType2",
    ]
    outputs: list["RunFile"]
    usage: "RunFinalType0Usage"
    ts_ms: int
    message: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.run_final_type_0_outcome_type_2 import RunFinalType0OutcomeType2
        from ..models.run_final_type_0_usage import RunFinalType0Usage
        from ..models.run_final_type_0_outcome_type_0 import RunFinalType0OutcomeType0
        from ..models.run_final_type_0_outcome_type_1 import RunFinalType0OutcomeType1
        from ..models.run_file import RunFile

        attempt = self.attempt

        outcome: dict[str, Any]
        if isinstance(self.outcome, RunFinalType0OutcomeType0):
            outcome = self.outcome.to_dict()
        elif isinstance(self.outcome, RunFinalType0OutcomeType1):
            outcome = self.outcome.to_dict()
        else:
            outcome = self.outcome.to_dict()

        outputs = []
        for outputs_item_data in self.outputs:
            outputs_item = outputs_item_data.to_dict()
            outputs.append(outputs_item)

        usage = self.usage.to_dict()

        ts_ms = self.ts_ms

        message: Union[None, str]
        message = self.message

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "attempt": attempt,
                "outcome": outcome,
                "outputs": outputs,
                "usage": usage,
                "tsMs": ts_ms,
                "message": message,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.run_final_type_0_outcome_type_2 import RunFinalType0OutcomeType2
        from ..models.run_final_type_0_usage import RunFinalType0Usage
        from ..models.run_final_type_0_outcome_type_0 import RunFinalType0OutcomeType0
        from ..models.run_final_type_0_outcome_type_1 import RunFinalType0OutcomeType1
        from ..models.run_file import RunFile

        d = dict(src_dict)
        attempt = d.pop("attempt")

        def _parse_outcome(
            data: object,
        ) -> Union[
            "RunFinalType0OutcomeType0",
            "RunFinalType0OutcomeType1",
            "RunFinalType0OutcomeType2",
        ]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                outcome_type_0 = RunFinalType0OutcomeType0.from_dict(data)

                return outcome_type_0
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                outcome_type_1 = RunFinalType0OutcomeType1.from_dict(data)

                return outcome_type_1
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            outcome_type_2 = RunFinalType0OutcomeType2.from_dict(data)

            return outcome_type_2

        outcome = _parse_outcome(d.pop("outcome"))

        outputs = []
        _outputs = d.pop("outputs")
        for outputs_item_data in _outputs:
            outputs_item = RunFile.from_dict(outputs_item_data)

            outputs.append(outputs_item)

        usage = RunFinalType0Usage.from_dict(d.pop("usage"))

        ts_ms = d.pop("tsMs")

        def _parse_message(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        message = _parse_message(d.pop("message"))

        run_final_type_0 = cls(
            attempt=attempt,
            outcome=outcome,
            outputs=outputs,
            usage=usage,
            ts_ms=ts_ms,
            message=message,
        )

        run_final_type_0.additional_properties = d
        return run_final_type_0

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
