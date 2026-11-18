from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.run_state import RunState
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.run_final_type_0 import RunFinalType0
    from ..models.run_attempts_item import RunAttemptsItem
    from ..models.run_queue import RunQueue
    from ..models.run_progress_type_0 import RunProgressType0
    from ..models.run_caps import RunCaps
    from ..models.run_file import RunFile


T = TypeVar("T", bound="Run")


@_attrs_define
class Run:
    """A compute run's record, without its arguments or keys: its method, state, caps (`fuel` a JSON number here), inputs,
    attempts (`tsMs` Unix milliseconds) and final outcome (`null` while it runs; `message` the failure text, `null`
    unless the caller may list the app's files). `progress` is the leader's last poll (`null` elsewhere, when it is not
    active, or to a caller who may not list files); `queue` where the run waits for a job node, to a caller who may list
    files only.

        Attributes:
            run (str):
            method (str):
            state (RunState):
            cancel_requested (bool):
            start_ts_ms (int):
            caps (RunCaps):
            inputs (list['RunFile']):
            attempts (list['RunAttemptsItem']):
            final (Union['RunFinalType0', None]):
            progress (Union['RunProgressType0', None]):
            queue (Union[Unset, RunQueue]):
    """

    run: str
    method: str
    state: RunState
    cancel_requested: bool
    start_ts_ms: int
    caps: "RunCaps"
    inputs: list["RunFile"]
    attempts: list["RunAttemptsItem"]
    final: Union["RunFinalType0", None]
    progress: Union["RunProgressType0", None]
    queue: Union[Unset, "RunQueue"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.run_final_type_0 import RunFinalType0
        from ..models.run_attempts_item import RunAttemptsItem
        from ..models.run_queue import RunQueue
        from ..models.run_progress_type_0 import RunProgressType0
        from ..models.run_caps import RunCaps
        from ..models.run_file import RunFile

        run = self.run

        method = self.method

        state = self.state.value

        cancel_requested = self.cancel_requested

        start_ts_ms = self.start_ts_ms

        caps = self.caps.to_dict()

        inputs = []
        for inputs_item_data in self.inputs:
            inputs_item = inputs_item_data.to_dict()
            inputs.append(inputs_item)

        attempts = []
        for attempts_item_data in self.attempts:
            attempts_item = attempts_item_data.to_dict()
            attempts.append(attempts_item)

        final: Union[None, dict[str, Any]]
        if isinstance(self.final, RunFinalType0):
            final = self.final.to_dict()
        else:
            final = self.final

        progress: Union[None, dict[str, Any]]
        if isinstance(self.progress, RunProgressType0):
            progress = self.progress.to_dict()
        else:
            progress = self.progress

        queue: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.queue, Unset):
            queue = self.queue.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "run": run,
                "method": method,
                "state": state,
                "cancelRequested": cancel_requested,
                "startTsMs": start_ts_ms,
                "caps": caps,
                "inputs": inputs,
                "attempts": attempts,
                "final": final,
                "progress": progress,
            }
        )
        if queue is not UNSET:
            field_dict["queue"] = queue

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.run_final_type_0 import RunFinalType0
        from ..models.run_attempts_item import RunAttemptsItem
        from ..models.run_queue import RunQueue
        from ..models.run_progress_type_0 import RunProgressType0
        from ..models.run_caps import RunCaps
        from ..models.run_file import RunFile

        d = dict(src_dict)
        run = d.pop("run")

        method = d.pop("method")

        state = RunState(d.pop("state"))

        cancel_requested = d.pop("cancelRequested")

        start_ts_ms = d.pop("startTsMs")

        caps = RunCaps.from_dict(d.pop("caps"))

        inputs = []
        _inputs = d.pop("inputs")
        for inputs_item_data in _inputs:
            inputs_item = RunFile.from_dict(inputs_item_data)

            inputs.append(inputs_item)

        attempts = []
        _attempts = d.pop("attempts")
        for attempts_item_data in _attempts:
            attempts_item = RunAttemptsItem.from_dict(attempts_item_data)

            attempts.append(attempts_item)

        def _parse_final(data: object) -> Union["RunFinalType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                final_type_0 = RunFinalType0.from_dict(data)

                return final_type_0
            except:  # noqa: E722
                pass
            return cast(Union["RunFinalType0", None], data)

        final = _parse_final(d.pop("final"))

        def _parse_progress(data: object) -> Union["RunProgressType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                progress_type_0 = RunProgressType0.from_dict(data)

                return progress_type_0
            except:  # noqa: E722
                pass
            return cast(Union["RunProgressType0", None], data)

        progress = _parse_progress(d.pop("progress"))

        _queue = d.pop("queue", UNSET)
        queue: Union[Unset, RunQueue]
        if isinstance(_queue, Unset):
            queue = UNSET
        else:
            queue = RunQueue.from_dict(_queue)

        run = cls(
            run=run,
            method=method,
            state=state,
            cancel_requested=cancel_requested,
            start_ts_ms=start_ts_ms,
            caps=caps,
            inputs=inputs,
            attempts=attempts,
            final=final,
            progress=progress,
            queue=queue,
        )

        run.additional_properties = d
        return run

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
